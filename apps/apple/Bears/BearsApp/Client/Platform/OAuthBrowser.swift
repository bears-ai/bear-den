import AppKit
import AuthenticationServices
import BearsClient

@MainActor
protocol OAuthBrowser {
    func authorize(_ authorization: OAuthAuthorization) async throws -> AuthorizationCode
    func cancel()
}

@MainActor
final class SystemOAuthBrowser: NSObject, OAuthBrowser, ASWebAuthenticationPresentationContextProviding {
    static let callbackURL = URL(string: "bears://oauth/callback")!
    private var session: ASWebAuthenticationSession?
    private var continuation: CheckedContinuation<AuthorizationCode, Error>?
    private var requestID: UUID?
    private var anchor: NSWindow?
    private enum Outcome: Sendable {
        case authorized(AuthorizationCode), invalid(ClientError), failed(ClientPlatformError)
    }

    func authorize(_ authorization: OAuthAuthorization) async throws -> AuthorizationCode {
        try Task.checkCancellation()
        cancel()
        guard authorization.redirectURI == Self.callbackURL else { throw ClientError.invalidConfiguration }
        guard let window = NSApp.keyWindow ?? NSApp.mainWindow ?? NSApp.windows.first(where: \.isVisible) else {
            throw ClientPlatformError.noPresentationWindow
        }
        let id = UUID()
        let url = try authorization.authorizationURL()
        return try await withTaskCancellationHandler {
            try Task.checkCancellation()
            return try await withCheckedThrowingContinuation { continuation in
                self.continuation = continuation
                requestID = id
                anchor = window
                let session = ASWebAuthenticationSession(url: url, callbackURLScheme: "bears") { [weak self] callback, error in
                    let outcome: Outcome
                    if let callback {
                        do { outcome = .authorized(try authorization.validateCallback(callback)) }
                        catch let error as ClientError { outcome = .invalid(error) }
                        catch { outcome = .invalid(.invalidCallback) }
                    } else if let error = error as? ASWebAuthenticationSessionError, error.code == .canceledLogin {
                        outcome = .failed(.signInCancelled)
                    } else { outcome = .failed(.browserUnavailable) }
                    Task { @MainActor [weak self] in self?.finish(id: id, outcome: outcome) }
                }
                self.session = session
                session.presentationContextProvider = self
                session.prefersEphemeralWebBrowserSession = true
                if !session.start() { finish(id: id, outcome: .failed(.browserUnavailable)) }
            }
        } onCancel: { [weak self] in
            guard let browser = self else { return }
            Task { @MainActor in browser.cancel(id: id) }
        }
    }

    func cancel() { if let id = requestID { cancel(id: id) } }
    private func cancel(id: UUID) {
        guard requestID == id else { return }
        let previous = session
        finish(id: id, outcome: .failed(.signInCancelled))
        previous?.cancel()
    }
    private func finish(id: UUID, outcome: Outcome) {
        guard requestID == id else { return }
        let continuation = continuation
        self.continuation = nil; requestID = nil; session = nil; anchor = nil
        switch outcome {
        case .authorized(let code): continuation?.resume(returning: code)
        case .invalid(let error): continuation?.resume(throwing: error)
        case .failed(let error): continuation?.resume(throwing: error)
        }
    }
    func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        // authorize() retains a real presentation window throughout the session.
        anchor ?? NSApp.keyWindow ?? NSApp.mainWindow ?? ASPresentationAnchor()
    }
}
