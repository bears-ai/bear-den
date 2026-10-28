import Foundation
import Security
import CryptoKit
import XCTest
@testable import BearsClient
@testable import BearsApp

final class ClientPlatformTests: XCTestCase {
    func testPKCEUsesSecureRandomMaterialAndS256Base64URL() throws {
        let first = try PKCEGenerator.generate()
        let second = try PKCEGenerator.generate()
        XCTAssertEqual(first.pkce.verifier.utf8.count, 43)
        XCTAssertEqual(first.state.utf8.count, 43)
        XCTAssertNotEqual(first.pkce.verifier, second.pkce.verifier)
        XCTAssertNotEqual(first.state, second.state)
        let expected = Data(SHA256.hash(data: Data(first.pkce.verifier.utf8))).base64EncodedString()
            .replacingOccurrences(of: "+", with: "-").replacingOccurrences(of: "/", with: "_").replacingOccurrences(of: "=", with: "")
        XCTAssertEqual(first.pkce.challenge, expected)
    }
    @MainActor
    func testPreferencesPersistOnlyNonSecretConfiguration() async throws {
        let name = "BearsPreferencesTests.\(UUID())"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: name))
        defer { defaults.removePersistentDomain(forName: name) }
        let preferences = ClientPreferences(defaults: defaults)
        XCTAssertTrue(preferences.serverURL.isEmpty)
        XCTAssertTrue(preferences.publicClientID.isEmpty)
        let configuration = try NativeFixtures.configuration()
        preferences.save(configuration)
        XCTAssertEqual(preferences.serverURL, configuration.baseURL.absoluteString)
        XCTAssertEqual(preferences.publicClientID, "apple-test")
        let values = defaults.persistentDomain(forName: name) ?? [:]
        XCTAssertEqual(Set(values.keys), ["nativeClient.serverURL", "nativeClient.publicClientID"])
        XCTAssertFalse(String(describing: values).contains("test-only-sensitive-token"))
    }
    @MainActor
    func testKeychainIdentityIsScopedByDeploymentAndPublicClientID() async throws {
        let first = try NativeFixtures.configuration()
        let normalized = try NativeFixtures.configuration("den/")
        let otherDen = try NativeFixtures.configuration("other-den")
        let otherClient = try NativeFixtures.configuration(client: "another-client")
        XCTAssertEqual(KeychainTokenVault.account(for: first), KeychainTokenVault.account(for: normalized))
        XCTAssertNotEqual(KeychainTokenVault.account(for: first), KeychainTokenVault.account(for: otherDen))
        XCTAssertNotEqual(KeychainTokenVault.account(for: first), KeychainTokenVault.account(for: otherClient))
        XCTAssertFalse(KeychainTokenVault.account(for: first).contains("test.example"))
        XCTAssertEqual(SystemOAuthBrowser.callbackURL.absoluteString, "bears://oauth/callback")
    }
    @MainActor
    func testKeychainSaveLoadAndDeleteUseDeviceOnlyNonSynchronizingItems() async throws {
        let access = TestKeychainAccess()
        let vault = KeychainTokenVault(keychain: access)
        let configuration = try NativeFixtures.configuration()
        XCTAssertNil(try vault.load(for: configuration))
        let token = NativeFixtures.token
        try vault.save(token, for: configuration)
        XCTAssertEqual(access.lastQuery?[kSecAttrService as String] as? String, KeychainTokenVault.service)
        XCTAssertEqual(access.added?[kSecAttrSynchronizable as String] as? Bool, false)
        XCTAssertEqual(access.added?[kSecAttrAccessible as String] as? String, kSecAttrAccessibleWhenUnlockedThisDeviceOnly as String)
        XCTAssertEqual(try vault.load(for: configuration), token)
        try vault.remove(for: configuration)
        XCTAssertNil(try vault.load(for: configuration))
        XCTAssertNoThrow(try vault.remove(for: configuration))
    }
    @MainActor
    func testKeychainFailuresAndMalformedDataAreReportedNotTreatedAsSignedOut() async throws {
        let access = TestKeychainAccess()
        let vault = KeychainTokenVault(keychain: access)
        let configuration = try NativeFixtures.configuration()
        access.copyFailure = errSecAuthFailed
        XCTAssertThrowsError(try vault.load(for: configuration)) { XCTAssertEqual($0 as? ClientPlatformError, .keychain(errSecAuthFailed)) }
        access.copyFailure = nil
        access.data = Data("not a token".utf8)
        XCTAssertThrowsError(try vault.load(for: configuration)) { XCTAssertEqual($0 as? ClientPlatformError, .invalidStoredToken) }
        access.updateFailure = errSecInteractionNotAllowed
        XCTAssertThrowsError(try vault.save(NativeFixtures.token, for: configuration)) {
            XCTAssertEqual($0 as? ClientPlatformError, .keychain(errSecInteractionNotAllowed))
        }
        access.deleteFailure = errSecAuthFailed
        XCTAssertThrowsError(try vault.remove(for: configuration)) { XCTAssertEqual($0 as? ClientPlatformError, .keychain(errSecAuthFailed)) }
    }
}

@MainActor
private final class TestKeychainAccess: KeychainAccessing {
    var data: Data?
    var lastQuery: [String: Any]?
    var added: [String: Any]?
    var copyFailure: OSStatus?
    var updateFailure: OSStatus?
    var deleteFailure: OSStatus?
    func copy(_ query: [String: Any]) -> (OSStatus, Data?) {
        lastQuery = query
        if let copyFailure { return (copyFailure, nil) }
        return data == nil ? (errSecItemNotFound, nil) : (errSecSuccess, data)
    }
    func update(_ query: [String: Any], attributes: [String: Any]) -> OSStatus {
        lastQuery = query
        if let updateFailure { return updateFailure }
        guard data != nil else { return errSecItemNotFound }
        data = attributes[kSecValueData as String] as? Data
        return errSecSuccess
    }
    func add(_ attributes: [String: Any]) -> OSStatus {
        added = attributes
        data = attributes[kSecValueData as String] as? Data
        return errSecSuccess
    }
    func delete(_ query: [String: Any]) -> OSStatus {
        lastQuery = query
        if let deleteFailure { return deleteFailure }
        let existed = data != nil
        data = nil
        return existed ? errSecSuccess : errSecItemNotFound
    }
}
