import Foundation
import BearsClient

/// Only non-secret connection preferences belong in UserDefaults.
@MainActor
struct ClientPreferences {
    private let defaults: UserDefaults
    init(defaults: UserDefaults = .standard) { self.defaults = defaults }

    var serverURL: String { defaults.string(forKey: "nativeClient.serverURL") ?? "" }
    var publicClientID: String { defaults.string(forKey: "nativeClient.publicClientID") ?? "" }

    func save(_ configuration: ClientConfiguration) {
        defaults.set(configuration.baseURL.absoluteString, forKey: "nativeClient.serverURL")
        defaults.set(configuration.clientID, forKey: "nativeClient.publicClientID")
    }
}
