import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif
import XCTest
@testable import BearsClient

final class DenClientTests: XCTestCase {
    func testBearsUsesBearerAndDeploymentPrefix() async throws {
        let configuration = try ClientFixtures.configuration()
        let host = configuration.baseURL.host!
        defer { HTTPStub.registry.remove(host: host) }
        HTTPStub.registry.register(host: host) { request in
            XCTAssertEqual(request.url?.path, "/deployment/v1.0/bears")
            XCTAssertEqual(request.value(forHTTPHeaderField: "Authorization"), "Bearer test-token")
            XCTAssertNil(request.value(forHTTPHeaderField: "Cookie"))
            XCTAssertFalse(request.httpShouldHandleCookies)
            return (200, Data("""
            [{"bear_id":"\(ClientFixtures.uuid)","slug":"bear","name":"Bear","description":"Purpose","is_bear_admin":true}]
            """.utf8))
        }
        let client = DenClient(configuration: configuration, protocolClasses: [HTTPStub.self])
        let bears = try await client.bears(token: ClientFixtures.token)
        XCTAssertEqual(bears.first?.id, BearID(ClientFixtures.uuid))
        XCTAssertEqual(bears.first?.name, "Bear")
        XCTAssertEqual(bears.first?.isBearAdmin, true)
    }

    func testInventoryAndCreateShapes() async throws {
        let configuration = try ClientFixtures.configuration()
        let host = configuration.baseURL.host!
        defer { HTTPStub.registry.remove(host: host) }
        let input = CreateConversation(bearID: BearID(ClientFixtures.uuid), hatID: HatID(ClientFixtures.uuid), requestID: RequestID(ClientFixtures.uuid))
        HTTPStub.registry.register(host: host) { request in
            XCTAssertEqual(request.url?.path, "/deployment/v1.0/chat/conversations")
            if request.httpMethod == "POST" {
                XCTAssertEqual(request.value(forHTTPHeaderField: "Content-Type"), "application/json")
                XCTAssertEqual(try JSONDecoder().decode(CreateConversation.self, from: request.httpBody!), input)
                return (200, Data("""
                {"id":"conv-1","title":"New chat","hat_id":"\(ClientFixtures.uuid)",
                 "session_uri":"\(ClientFixtures.session.rawValue)","chat_uri":"\(ClientFixtures.chat.rawValue)"}
                """.utf8))
            }
            XCTAssertEqual(URLComponents(url: request.url!, resolvingAgainstBaseURL: false)?.queryItems?.first?.name, "bear_id")
            return (200, Data("""
            {"conversations":[{"id":"conv-1","title":"Chat","hat_id":null,"can_send":false,
             "session_uri":"\(ClientFixtures.session.rawValue)","chat_uri":"\(ClientFixtures.chat.rawValue)"}],
             "hats":[{"id":"\(ClientFixtures.uuid)","name":"Pair","short_summary":"A hat"}]}
            """.utf8))
        }
        let client = DenClient(configuration: configuration, protocolClasses: [HTTPStub.self])
        let inventory = try await client.conversations(bearID: input.bearID, token: ClientFixtures.token)
        XCTAssertFalse(inventory.conversations[0].canSend)
        XCTAssertNil(inventory.conversations[0].lastMessageAt)
        XCTAssertNil(inventory.conversations[0].hatID)
        XCTAssertEqual(inventory.hats[0].shortSummary, "A hat")
        let created = try await client.createConversation(input, token: ClientFixtures.token)
        XCTAssertEqual(created.chatURI, ClientFixtures.chat)
        XCTAssertEqual(created.id, ConversationID("conv-1"))
    }

    func testHistoryPaginationAndEncodedConversationID() async throws {
        let configuration = try ClientFixtures.configuration()
        let host = configuration.baseURL.host!
        defer { HTTPStub.registry.remove(host: host) }
        HTTPStub.registry.register(host: host) { request in
            XCTAssertEqual(request.url?.path, "/deployment/v1.0/chat/history")
            let query = URLComponents(url: request.url!, resolvingAgainstBaseURL: false)!.queryItems!
            XCTAssertEqual(query.first { $0.name == "conversation_id" }?.value, "conv/&? +")
            XCTAssertTrue(request.url!.absoluteString.contains("%2B"))
            XCTAssertEqual(query.first { $0.name == "before" }?.value, "42")
            XCTAssertEqual(query.first { $0.name == "limit" }?.value, "10")
            return (200, Data("""
            {"messages":[{"id":"message:41","sequence_no":41,"role":"assistant","kind":"message", "text":"**markdown**","created_at":"2026-10-28T01:00:00.123Z"}],"has_more":true,"next_before":"41"}
            """.utf8))
        }
        let client = DenClient(configuration: configuration, protocolClasses: [HTTPStub.self])
        let page = try await client.history(bearID: BearID(ClientFixtures.uuid), conversationID: ConversationID("conv/&? +"), before: 42, limit: 10, token: ClientFixtures.token)
        XCTAssertEqual(page.beforeSequence, 41)
        XCTAssertTrue(page.hasMore)
        XCTAssertEqual(page.messages[0].role, .assistant)
        XCTAssertEqual(page.messages[0].text, "**markdown**")
        do {
            _ = try await client.history(bearID: BearID(ClientFixtures.uuid), conversationID: ConversationID("conv-1"), limit: 101, token: ClientFixtures.token)
            XCTFail("Invalid history bounds must not make a request")
        } catch { XCTAssertEqual(error as? ClientError, .invalidInput) }
    }

    func testPublicPKCEFormAndTokenExpiry() async throws {
        let configuration = try ClientFixtures.configuration()
        let authorization = try ClientFixtures.authorization(configuration: configuration)
        let code = try authorization.validateCallback(URL(string: "bears-test://oauth/callback?state=test-state&code=a%2Bb%26c")!)
        let host = configuration.baseURL.host!
        defer { HTTPStub.registry.remove(host: host) }
        HTTPStub.registry.register(host: host) { request in
            XCTAssertEqual(request.httpMethod, "POST")
            XCTAssertEqual(request.url?.path, "/deployment/oauth/token")
            XCTAssertNil(request.value(forHTTPHeaderField: "Authorization"))
            XCTAssertEqual(request.value(forHTTPHeaderField: "Content-Type"), "application/x-www-form-urlencoded")
            let form = String(data: request.httpBody!, encoding: .utf8)!
            XCTAssertTrue(form.contains("grant_type=authorization_code"))
            XCTAssertTrue(form.contains("code=a%2Bb%26c"))
            XCTAssertTrue(form.contains("client_id=apple-test"))
            XCTAssertTrue(form.contains("code_verifier=" + String(repeating: "v", count: 43)))
            XCTAssertFalse(form.contains("client_secret"))
            return (200, Data("""
            {"access_token":"new-token","token_type":"Bearer","expires_in":3600,"scope":"data:read"}
            """.utf8))
        }
        let started = Date()
        let client = DenClient(configuration: configuration, protocolClasses: [HTTPStub.self])
        let token = try await client.exchangeCode(code, authorization: authorization)
        XCTAssertEqual(token.accessToken.rawValue, "new-token")
        XCTAssertEqual(token.scope, "data:read")
        XCTAssertGreaterThanOrEqual(token.expiresAt.timeIntervalSince(started), 3599)
        XCTAssertLessThanOrEqual(token.expiresAt.timeIntervalSinceNow, 3600)
    }

    func testUserInfoShapeAndExpiredTokenNeverSends() async throws {
        let configuration = try ClientFixtures.configuration()
        let host = configuration.baseURL.host!
        defer { HTTPStub.registry.remove(host: host) }
        HTTPStub.registry.register(host: host) { request in
            XCTAssertEqual(request.url?.path, "/deployment/oauth/userinfo")
            return (200, Data("{\"sub\":\"42\",\"preferred_username\":\"human\",\"name\":null,\"email\":null}".utf8))
        }
        let client = DenClient(configuration: configuration, protocolClasses: [HTTPStub.self])
        let info = try await client.userInfo(token: ClientFixtures.token)
        XCTAssertEqual(info.subject, SubjectID("42"))
        XCTAssertEqual(info.preferredUsername, "human")
        let expired = OAuthToken(accessToken: ClientFixtures.token.accessToken, expiresAt: .distantPast, scope: "")
        HTTPStub.registry.register(host: host) { _ in XCTFail("Expired credentials must not leave the client"); return (200, Data()) }
        do { _ = try await client.bears(token: expired); XCTFail("Expected expiry") }
        catch { XCTAssertEqual(error as? ClientError, .expiredToken) }
    }

    func testServerErrorsAndMalformedResponsesAreSafe() async throws {
        let configuration = try ClientFixtures.configuration()
        let host = configuration.baseURL.host!
        defer { HTTPStub.registry.remove(host: host) }
        let client = DenClient(configuration: configuration, protocolClasses: [HTTPStub.self])
        for (status, body, expected) in [
            (403, "{\"error\":\"forbidden\",\"message\":\"secret test-token\"}", ClientError.http(status: 403, code: .forbidden)),
            (502, "<html>secret test-token</html>", ClientError.http(status: 502, code: .unknown)),
            (400, "{\"error\":\"secret test-token\",\"message\":\"secret\"}", ClientError.http(status: 400, code: .unknown)),
            (200, "{\"secret\":\"test-token\"}", ClientError.invalidResponse),
            (302, "", ClientError.http(status: 302, code: .unknown))
        ] {
            HTTPStub.registry.register(host: host) { _ in (status, Data(body.utf8)) }
            do { _ = try await client.bears(token: ClientFixtures.token); XCTFail("Expected safe failure") }
            catch {
                XCTAssertEqual(error as? ClientError, expected)
                XCTAssertFalse(String(reflecting: error).contains("secret"))
                XCTAssertFalse(error.localizedDescription.contains("test-token"))
            }
        }
        HTTPStub.registry.register(host: host) { _ in throw URLError(.cannotConnectToHost, userInfo: [NSLocalizedDescriptionKey: "secret test-token"]) }
        do { _ = try await client.bears(token: ClientFixtures.token); XCTFail("Expected transport failure") }
        catch { XCTAssertEqual(error as? ClientError, .transport) }
    }

    func testAlreadyCancelledRequestNeverSends() async throws {
        let configuration = try ClientFixtures.configuration()
        let host = configuration.baseURL.host!
        defer { HTTPStub.registry.remove(host: host) }
        HTTPStub.registry.register(host: host) { _ in
            XCTFail("An already-cancelled operation must not start a network task")
            return (200, Data("[]".utf8))
        }
        let client = DenClient(configuration: configuration, protocolClasses: [HTTPStub.self])
        let operation = Task {
            withUnsafeCurrentTask { $0?.cancel() }
            return try await client.bears(token: ClientFixtures.token)
        }
        do { _ = try await operation.value; XCTFail("Expected cancellation") }
        catch { XCTAssertEqual(error as? ClientError, .cancelled) }
    }

    func testOversizedResponseIsRejectedWithoutExposingBody() async throws {
        let configuration = try ClientFixtures.configuration()
        let host = configuration.baseURL.host!
        defer { HTTPStub.registry.remove(host: host) }
        HTTPStub.registry.register(host: host) { _ in (500, Data(repeating: 65, count: 4 * 1024 * 1024 + 1)) }
        let client = DenClient(configuration: configuration, protocolClasses: [HTTPStub.self])
        do { _ = try await client.bears(token: ClientFixtures.token); XCTFail("Expected size limit") }
        catch { XCTAssertEqual(error as? ClientError, .responseTooLarge) }
    }

    func testEphemeralSessionSafetyConfigurationAndRedirectDelegate() {
        let session = ClientSession.make()
        defer { session.invalidateAndCancel() }
        XCTAssertFalse(session.configuration.httpShouldSetCookies)
        XCTAssertNil(session.configuration.httpCookieStorage)
        XCTAssertNil(session.configuration.urlCache)
        XCTAssertNil(session.configuration.urlCredentialStorage)
        let request = URLRequest(url: URL(string: "https://example.test")!)
        let task = session.dataTask(with: request)
        let response = HTTPURLResponse(url: request.url!, statusCode: 302, httpVersion: nil, headerFields: nil)!
        var called = false
        SessionSafetyDelegate().urlSession(session, task: task, willPerformHTTPRedirection: response, newRequest: request) { redirected in
            called = true
            XCTAssertNil(redirected)
        }
        XCTAssertTrue(called)
        task.cancel()
    }
}
