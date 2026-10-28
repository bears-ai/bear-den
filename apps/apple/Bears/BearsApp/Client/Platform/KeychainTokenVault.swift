import Foundation
import Security
import CryptoKit
import BearsClient

@MainActor
protocol TokenVault {
    func load(for configuration: ClientConfiguration) throws -> OAuthToken?
    func save(_ token: OAuthToken, for configuration: ClientConfiguration) throws
    func remove(for configuration: ClientConfiguration) throws
}

@MainActor
protocol KeychainAccessing {
    func copy(_ query: [String: Any]) -> (OSStatus, Data?)
    func update(_ query: [String: Any], attributes: [String: Any]) -> OSStatus
    func add(_ attributes: [String: Any]) -> OSStatus
    func delete(_ query: [String: Any]) -> OSStatus
}

@MainActor
struct SystemKeychain: KeychainAccessing {
    func copy(_ query: [String: Any]) -> (OSStatus, Data?) {
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        return (status, result as? Data)
    }
    func update(_ query: [String: Any], attributes: [String: Any]) -> OSStatus {
        SecItemUpdate(query as CFDictionary, attributes as CFDictionary)
    }
    func add(_ attributes: [String: Any]) -> OSStatus { SecItemAdd(attributes as CFDictionary, nil) }
    func delete(_ query: [String: Any]) -> OSStatus { SecItemDelete(query as CFDictionary) }
}

@MainActor
struct KeychainTokenVault: TokenVault {
    static let service = "ai.bears.native.oauth-token"
    private let keychain: any KeychainAccessing
    init() { keychain = SystemKeychain() }
    init(keychain: any KeychainAccessing) { self.keychain = keychain }

    static func account(for configuration: ClientConfiguration) -> String {
        // An encoded tuple avoids ambiguous concatenation; the hash avoids publishing a Den URL in item labels.
        let identity = try! JSONEncoder().encode([configuration.baseURL.absoluteString, configuration.clientID])
        return SHA256.hash(data: identity).map { String(format: "%02x", $0) }.joined()
    }
    private func query(_ configuration: ClientConfiguration) -> [String: Any] {
        [kSecClass as String: kSecClassGenericPassword,
         kSecAttrService as String: Self.service,
         kSecAttrAccount as String: Self.account(for: configuration),
         kSecAttrSynchronizable as String: false]
    }
    func load(for configuration: ClientConfiguration) throws -> OAuthToken? {
        var query = query(configuration)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        let (status, data) = keychain.copy(query)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess else { throw ClientPlatformError.keychain(status) }
        guard let data, let token = try? JSONDecoder().decode(OAuthToken.self, from: data) else {
            throw ClientPlatformError.invalidStoredToken
        }
        return token
    }
    func save(_ token: OAuthToken, for configuration: ClientConfiguration) throws {
        let data = try JSONEncoder().encode(token)
        let query = query(configuration)
        let attributes: [String: Any] = [kSecValueData as String: data,
                                       kSecAttrAccessible as String: kSecAttrAccessibleWhenUnlockedThisDeviceOnly]
        let status = keychain.update(query, attributes: attributes)
        if status == errSecItemNotFound {
            let added = keychain.add(query.merging(attributes) { _, new in new })
            guard added == errSecSuccess else { throw ClientPlatformError.keychain(added) }
        } else if status != errSecSuccess { throw ClientPlatformError.keychain(status) }
    }
    func remove(for configuration: ClientConfiguration) throws {
        let status = keychain.delete(query(configuration))
        guard status == errSecSuccess || status == errSecItemNotFound else { throw ClientPlatformError.keychain(status) }
    }
}
