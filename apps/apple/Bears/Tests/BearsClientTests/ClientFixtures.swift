import Foundation
@testable import BearsClient

// All URLs here are test fixtures, never defaults for a production client.
enum ClientFixtures {
    static let uuid = UUID(uuidString: "12345678-1234-4234-8234-123456789abc")!
    static let turn = "23456789-1234-4234-8234-123456789abc"
    static var chat: ChatURI { try! ChatURI(uuid) }
    static var session: SessionURI { try! SessionURI(uuid) }
    static var token: OAuthToken {
        OAuthToken(accessToken: try! AccessToken("test-token"), expiresAt: Date().addingTimeInterval(3600), scope: "data:read data:write")
    }
    static func configuration() throws -> ClientConfiguration {
        try ClientConfiguration(baseURL: URL(string: "https://\(UUID().uuidString.lowercased()).example/deployment")!, clientID: "apple-test")
    }
    static func authorization(configuration: ClientConfiguration? = nil) throws -> OAuthAuthorization {
        try OAuthAuthorization(configuration: configuration ?? self.configuration(),
            redirectURI: URL(string: "bears-test://oauth/callback")!, state: "test-state",
            pkce: PKCEMaterial(verifier: String(repeating: "v", count: 43), challenge: String(repeating: "c", count: 43)))
    }
    static func snapshot(seq: UInt64 = 10, turns: String = "[]", active: String? = nil) throws -> ChatSnapshot {
        let live = active.map { ",\"activeTurn\":\($0)" } ?? ""
        let json = """
        {"resource":"\(chat.rawValue)","fromSeq":\(seq),"state":{
          "resource":"\(chat.rawValue)","title":"Chat","status":1,"modifiedAt":"2026-10-28T00:00:00Z",
          "interactivity":"full","turns":\(turns)\(live)}}
        """
        guard case .chat(let snapshot) = try JSONDecoder().decode(WireSnapshot.self, from: Data(json.utf8)) else {
            throw ClientError.invalidResponse
        }
        return snapshot
    }
    static func action(_ body: String, seq: UInt64, rejection: String? = nil, channel: String? = nil) throws -> AHPActionEnvelope {
        let reason = rejection.map { ",\"rejectionReason\":\"\($0)\"" } ?? ""
        let json = """
        {"channel":"\(channel ?? chat.rawValue)","serverSeq":\(seq),"action":\(body),
         "origin":{"clientId":"apple-test","clientSeq":1}\(reason)}
        """
        return try JSONDecoder().decode(AHPActionEnvelope.self, from: Data(json.utf8))
    }
    static var start: String {
        """
        {"type":"chat/turnStarted","turnId":"\(turn)","startedAt":"2026-10-28T00:00:00Z",
         "message":{"text":"Hello","origin":{"kind":"user"}}}
        """
    }
    static var part: String {
        """
        {"type":"chat/responsePart","turnId":"\(turn)","part":{"kind":"markdown","id":"response-1","content":""}}
        """
    }
    static var delta: String {
        """
        {"type":"chat/delta","turnId":"\(turn)","partId":"response-1","content":"**Hello**"}
        """
    }
}
