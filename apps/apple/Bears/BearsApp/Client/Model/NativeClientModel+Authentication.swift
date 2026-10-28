import Foundation
import BearsClient

@MainActor
extension NativeClientModel {
    func signIn() {
        guard let configuration, authentication != .signingIn else { return }
        let heldDrafts = authentication == .needsReauthorization ? reauthorizationDrafts : nil
        resetRuntime()
        reauthorizationDrafts = heldDrafts
        let generation = sessionGeneration
        let api = dependencies.makeAPI(configuration)
        self.api = api
        authentication = .signingIn
        issue = nil
        authTask = Task {
            guard sessionGeneration == generation, !Task.isCancelled else { return }
            do {
                let material = try dependencies.loginMaterial()
                let authorization = try OAuthAuthorization(configuration: configuration,
                    redirectURI: SystemOAuthBrowser.callbackURL, state: material.state, pkce: material.pkce)
                let code = try await dependencies.browser.authorize(authorization)
                guard sessionGeneration == generation, !Task.isCancelled else { return }
                let token = try await api.exchangeCode(code, authorization: authorization)
                guard sessionGeneration == generation, !Task.isCancelled else { return }
                // Do not claim a persistent login unless its Keychain write actually succeeded.
                try dependencies.vault.save(token, for: configuration)
                failedSignOuts.removeAll { $0 == configuration }
                acceptToken(token)
            } catch {
                guard sessionGeneration == generation, !Task.isCancelled else { return }
                authentication = reauthorizationDrafts == nil ? .signedOut : .needsReauthorization
                issue = Self.safeMessage(error)
            }
        }
    }
    func cancelSignIn() {
        guard authentication == .signingIn else { return }
        let heldDrafts = reauthorizationDrafts
        resetRuntime()
        reauthorizationDrafts = heldDrafts
        if heldDrafts != nil { authentication = .needsReauthorization }
        issue = "Sign-in was cancelled."
    }
    func signOut() {
        let previous = configuration
        resetRuntime()
        issue = nil
        guard let previous else { return }
        do {
            try dependencies.vault.remove(for: previous)
            failedSignOuts.removeAll { $0 == previous }
        } catch {
            if !failedSignOuts.contains(previous) { failedSignOuts.append(previous) }
            issue = "Signed out of the running client, but the saved credential could not be removed. " + Self.safeMessage(error)
        }
    }
    func retryKeychainRemoval() {
        var remaining: [ClientConfiguration] = []
        issue = nil
        for failed in failedSignOuts {
            do { try dependencies.vault.remove(for: failed) }
            catch { remaining.append(failed); issue = Self.safeMessage(error) }
        }
        failedSignOuts = remaining
    }
    func requireReauthorization() {
        var held: ReauthorizationDrafts?
        if let configuration, let subject = user?.subject {
            var drafts = savedComposers
            if let selectedConversation {
                var current = composer
                current.disconnected()
                drafts[selectedConversation.chatURI] = current
            }
            held = ReauthorizationDrafts(configuration: configuration, subject: subject, composers: drafts)
        }
        resetRuntime()
        reauthorizationDrafts = held
        authentication = .needsReauthorization
    }
    func restoreReauthorizationDrafts(for subject: SubjectID) {
        guard let held = reauthorizationDrafts else { return }
        reauthorizationDrafts = nil
        guard held.configuration == configuration, held.subject == subject else { return }
        savedComposers = held.composers
        if let selectedConversation, composer.draft.isEmpty, !composer.hasPendingSend,
           let saved = savedComposers.removeValue(forKey: selectedConversation.chatURI) { composer = saved }
    }
    func acceptToken(_ token: OAuthToken) {
        self.token = token
        authentication = .signedIn
        let generation = sessionGeneration
        let lifetime = max(0, min(token.expiresAt.timeIntervalSinceNow, 31_536_000))
        expiryTask?.cancel()
        expiryTask = Task {
            do { try await Task.sleep(nanoseconds: UInt64(lifetime * 1_000_000_000)) } catch { return }
            guard sessionGeneration == generation else { return }
            requireReauthorization()
            issue = "Your access token expired. Sign in again to continue; this client does not refresh tokens."
        }
        refreshMembership()
    }
    func refreshMembership() {
        guard let api, let token, authentication == .signedIn else { return }
        catalogTask?.cancel()
        let generation = sessionGeneration
        isLoadingMembership = true
        catalogTask = Task {
            do {
                let members = try await api.bears(token: token)
                guard sessionGeneration == generation, !Task.isCancelled else { return }
                bears = members
                isLoadingMembership = false
                if let selectedBearID, !members.contains(where: { $0.id == selectedBearID }) { selectBear(nil) }
                if selectedBearID == nil, let first = members.first { selectBear(first.id) }
                let profile = try await api.userInfo(token: token)
                guard sessionGeneration == generation, !Task.isCancelled else { return }
                user = profile
                restoreReauthorizationDrafts(for: profile.subject)
            } catch {
                guard sessionGeneration == generation, !Task.isCancelled else { return }
                isLoadingMembership = false
                report(error)
            }
        }
    }
}
