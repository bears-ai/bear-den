import Foundation
#if canImport(FoundationNetworking)
@preconcurrency import FoundationNetworking
#endif

/// Reject all redirects, including same-origin ones, rather than risk forwarding credentials.
final class SessionSafetyDelegate: NSObject, URLSessionTaskDelegate, Sendable {
    func urlSession(_ session: URLSession, task: URLSessionTask,
                    willPerformHTTPRedirection response: HTTPURLResponse, newRequest request: URLRequest,
                    completionHandler: @escaping (URLRequest?) -> Void) { completionHandler(nil) }

    func urlSession(_ session: URLSession, task: URLSessionTask, didReceive challenge: URLAuthenticationChallenge,
                    completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void) {
        #if canImport(Security)
        if challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust {
            completionHandler(.performDefaultHandling, nil)
            return
        }
        #endif
        completionHandler(.cancelAuthenticationChallenge, nil)
    }
}

enum ClientSession {
    static func make(protocolClasses: [AnyClass]? = nil) -> URLSession {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.httpShouldSetCookies = false
        configuration.httpCookieStorage = nil
        configuration.urlCache = nil
        configuration.urlCredentialStorage = nil
        configuration.requestCachePolicy = .reloadIgnoringLocalCacheData
        configuration.timeoutIntervalForRequest = 30
        configuration.timeoutIntervalForResource = 60
        if let protocolClasses { configuration.protocolClasses = protocolClasses }
        return URLSession(configuration: configuration, delegate: SessionSafetyDelegate(), delegateQueue: nil)
    }
}

struct HTTPTransport: Sendable {
    let session: URLSession
    func execute<Response: Decodable & Sendable>(_ request: URLRequest) async throws -> Response {
        do {
            let transfer = HTTPTransfer()
            let (data, response) = try await withTaskCancellationHandler {
                try await withCheckedThrowingContinuation { continuation in
                    transfer.start(request: request, session: session, continuation: continuation)
                }
            } onCancel: { transfer.cancel() }
            try Task.checkCancellation()
            guard let http = response as? HTTPURLResponse else { throw ClientError.invalidResponse }
            guard data.count <= 4 * 1024 * 1024 else { throw ClientError.responseTooLarge }
            guard (200..<300).contains(http.statusCode) else {
                // Error text is untrusted and may contain credentials or proxy HTML. Retain only a known code.
                let failure = data.count <= 16_384 ? try? JSONDecoder().decode(HTTPFailure.self, from: data) : nil
                throw ClientError.http(status: http.statusCode, code: ServerErrorCode(rawValue: failure?.error ?? "") ?? .unknown)
            }
            do { return try JSONDecoder().decode(Response.self, from: data) }
            catch { throw ClientError.invalidResponse }
        } catch let error as ClientError { throw error }
        catch is CancellationError { throw ClientError.cancelled }
        catch let error as URLError where error.code == .cancelled { throw ClientError.cancelled }
        catch { throw ClientError.transport }
    }
}

private struct HTTPFailure: Decodable { let error: String; let message: String? }

/// The lock bridges cancellation to the callback-based API, also available in Linux Swift 5.10.
private final class HTTPTransfer: @unchecked Sendable {
    private let lock = NSLock()
    private var task: URLSessionDataTask?
    private var cancelled = false

    func start(request: URLRequest, session: URLSession,
               continuation: CheckedContinuation<(Data, URLResponse), Error>) {
        lock.lock()
        guard !cancelled else {
            lock.unlock()
            continuation.resume(throwing: ClientError.cancelled)
            return
        }
        let task = session.dataTask(with: request) { data, response, error in
            if let error { continuation.resume(throwing: error) }
            else if let data, let response { continuation.resume(returning: (data, response)) }
            else { continuation.resume(throwing: ClientError.invalidResponse) }
        }
        self.task = task
        lock.unlock()
        task.resume()
    }
    func cancel() {
        lock.lock()
        cancelled = true
        let task = task
        lock.unlock()
        task?.cancel()
    }
}

func formEncoded(_ pairs: [(String, String)]) -> Data {
    let allowed = CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._~")
    return Data(pairs.map { key, value in
        key.addingPercentEncoding(withAllowedCharacters: allowed)! + "=" + value.addingPercentEncoding(withAllowedCharacters: allowed)!
    }.joined(separator: "&").utf8)
}
