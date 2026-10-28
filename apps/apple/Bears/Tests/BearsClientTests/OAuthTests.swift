import Foundation
import XCTest
@testable import BearsClient

final class OAuthTests: XCTestCase {
    func testDeploymentPrefixAndPKCEAuthorization() throws {
        let authorization = try ClientFixtures.authorization()
        let url = try authorization.authorizationURL()
        XCTAssertEqual(url.path, "/deployment/oauth/authorize")
        let query = URLComponents(url: url, resolvingAgainstBaseURL: false)!.queryItems!
        func value(_ name: String) -> String? { query.first { $0.name == name }?.value }
        XCTAssertEqual(value("response_type"), "code")
        XCTAssertEqual(value("client_id"), "apple-test")
        XCTAssertEqual(value("redirect_uri"), "bears-test://oauth/callback")
        XCTAssertEqual(value("code_challenge_method"), "S256")
        XCTAssertEqual(value("code_challenge"), authorization.pkce.challenge)
        XCTAssertNil(value("code_verifier"))
        XCTAssertEqual(value("state"), "test-state")
        XCTAssertEqual(try authorization.configuration.webSocketURL().path, "/deployment/v1.0/agent-host")
        XCTAssertEqual(try authorization.configuration.webSocketURL().scheme, "wss")
    }

    func testCallbackRequiresExactDestinationAndStateEvenForErrors() throws {
        let authorization = try ClientFixtures.authorization()
        XCTAssertEqual(try authorization.validateCallback(URL(string: "bears-test://oauth/callback?state=test-state&code=a%2Bb")!).rawValue, "a+b")
        for url in [
            "other://oauth/callback?state=test-state&code=x",
            "bears-test://other/callback?state=test-state&code=x",
            "bears-test://oauth/other?state=test-state&code=x",
            "bears-test://oauth/callback/extra?state=test-state&code=x",
            "bears-test://oauth:99/callback?state=test-state&code=x",
            "bears-test://user@oauth/callback?state=test-state&code=x",
            "bears-test://oauth/%63allback?state=test-state&code=x",
            "bears-test://oauth/callback?state=wrong&code=x",
            "bears-test://oauth/callback?code=x",
            "bears-test://oauth/callback?state=test-state&state=test-state&code=x",
            "bears-test://oauth/callback?state=test-state&code=x&code=y",
            "bears-test://oauth/callback?state=test-state&code=x&error=denied",
            "bears-test://oauth/callback?state=test-state&code=x#fragment",
            "bears-test://other/callback?state=test-state&error=access_denied",
            "bears-test://oauth/callback?state=wrong&error=access_denied"
        ] {
            XCTAssertThrowsError(try authorization.validateCallback(URL(string: url)!)) {
                XCTAssertEqual($0 as? ClientError, .invalidCallback, url)
            }
        }
        XCTAssertThrowsError(try authorization.validateCallback(URL(string: "bears-test://oauth/callback?state=test-state&error=access_denied&error_description=secret")!)) {
            XCTAssertEqual($0 as? ClientError, .authorizationDenied)
            XCTAssertFalse(String(describing: $0).contains("secret"))
        }
    }

    func testHTTPSExceptExactLoopbackDevelopmentHosts() throws {
        for url in ["https://den.example/prefix", "http://localhost:3000/prefix", "http://127.0.0.1:3000"] {
            XCTAssertNoThrow(try ClientConfiguration(baseURL: URL(string: url)!, clientID: "apple"))
        }
        for url in ["http://den.example", "http://localhost.evil.example", "http://127.0.0.2", "ftp://localhost", "https://user:secret@den.example", "https://den.example?token=secret", "https://den.example#secret"] {
            XCTAssertThrowsError(try ClientConfiguration(baseURL: URL(string: url)!, clientID: "apple"))
        }
        XCTAssertThrowsError(try ClientConfiguration(baseURL: URL(string: "https://den.example")!, clientID: "\r\n"))
        XCTAssertThrowsError(try PKCEMaterial(verifier: "short", challenge: String(repeating: "c", count: 43)))
        XCTAssertThrowsError(try AccessToken("token\r\nInjected: value"))
    }

    func testTokenExpiryAndRedactedDescriptions() throws {
        let now = Date(timeIntervalSince1970: 1_000)
        let token = OAuthToken(accessToken: try AccessToken("sensitive-token"), expiresAt: now, scope: "data:read")
        XCTAssertTrue(token.isExpired(at: now))
        XCTAssertFalse(token.isExpired(at: now.addingTimeInterval(-1)))
        XCTAssertThrowsError(try token.authorize(at: now)) { XCTAssertEqual($0 as? ClientError, .expiredToken) }
        XCTAssertFalse(String(reflecting: token).contains("sensitive-token"))
        XCTAssertFalse(String(reflecting: token.accessToken).contains("sensitive-token"))
        XCTAssertFalse(String(reflecting: try ClientFixtures.authorization()).contains("test-state"))
    }

    func testIdentifiersAreSingleValueCodableAndResourcesCanonical() throws {
        let encoded = try JSONEncoder().encode(BearID(ClientFixtures.uuid))
        XCTAssertEqual(try JSONDecoder().decode(BearID.self, from: encoded), BearID(ClientFixtures.uuid))
        XCTAssertThrowsError(try ChatURI(rawValue: "ahp-chat:/12345678-1234-4234-8234-123456789ABC"))
        XCTAssertThrowsError(try ChatURI(rawValue: "ahp-chat:/00000000-0000-0000-0000-000000000000"))
        XCTAssertThrowsError(try ChatURI(rawValue: ClientFixtures.session.rawValue))
        XCTAssertEqual(try JSONDecoder().decode(ChatURI.self, from: JSONEncoder().encode(ClientFixtures.chat)), ClientFixtures.chat)
    }
}
