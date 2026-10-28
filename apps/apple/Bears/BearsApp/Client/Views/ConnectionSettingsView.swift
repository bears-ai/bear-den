import SwiftUI
import BearsClient

@MainActor
struct ConnectionSettingsView: View {
    @ObservedObject var model: NativeClientModel

    var body: some View {
        Form {
            Section("Den connection") {
                TextField("Den API URL", text: $model.serverURLText, prompt: Text("HTTPS deployment URL"))

                    .autocorrectionDisabled()
                TextField("Public OAuth client ID", text: $model.publicClientIDText)
                    .autocorrectionDisabled()
                Text("Use the deployment root, including any path prefix. HTTPS is required except for localhost or 127.0.0.1 development. Saving a connection closes the active chat and reloads its saved sign-in.")
                    .font(.caption).foregroundStyle(.secondary)
                HStack {
                    Button("Save connection") { model.configure() }
                        .disabled(model.authentication == .signingIn)
                    if model.configuration != nil { Text("Saved locally · no embedded server").font(.caption).foregroundStyle(.secondary) }
                }
            }
            Section("Sign in") {
                Text("Register this public client with redirect URI bears://oauth/callback. Login opens the system authentication browser with PKCE.")
                    .font(.caption).textSelection(.enabled)
                HStack {
                    if model.authentication == .signingIn {
                        ProgressView().controlSize(.small)
                        Text("Waiting for the browser…")
                        Button("Cancel") { model.cancelSignIn() }
                    } else if model.authentication == .signedIn {
                        Label(model.accountName, systemImage: "person.crop.circle")
                        Spacer()
                        Button("Sign out") { model.signOut() }
                    } else {
                        Button(model.authentication == .needsReauthorization ? "Sign in again" : "Sign in with Den") { model.signIn() }
                            .buttonStyle(.borderedProminent)
                            .disabled(!configurationMatchesDraft)
                    }
                }
                if model.authentication == .needsReauthorization {
                    Text("The access token expired or is no longer valid. Reauthorize to continue; refresh tokens are not used.")
                        .font(.caption).foregroundStyle(.secondary)
                }
                Text("Connection preferences are stored in UserDefaults. Tokens are stored only in Keychain, separately for each Den URL and public client ID.")
                    .font(.caption).foregroundStyle(.secondary)
                if model.configuration != nil && model.authentication != .signedIn && model.authentication != .signingIn {
                    Button("Remove saved sign-in") { model.signOut() }
                }
            }
            if model.failedSignOut != nil {
                Section("Keychain removal needs attention") {
                    Text("The running session is signed out, but a saved credential could not be removed. Retry removal before leaving this Mac unattended.")
                    Button("Retry Keychain removal") { model.retryKeychainRemoval() }
                }
            }
            if let issue = model.issue {
                Text(issue).foregroundStyle(.red).font(.callout).textSelection(.enabled)
            }
        }
        .formStyle(.grouped)
        .frame(minWidth: 480)
    }
    private var configurationMatchesDraft: Bool {
        guard let url = URL(string: model.serverURLText.trimmingCharacters(in: .whitespacesAndNewlines)),
              let draft = try? ClientConfiguration(baseURL: url, clientID: model.publicClientIDText.trimmingCharacters(in: .whitespacesAndNewlines)) else { return false }
        return model.configuration == draft
    }
}
