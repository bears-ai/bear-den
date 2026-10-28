import Foundation

public enum ChatInteractivity: String, Codable, Sendable { case full, readOnly = "read-only" }
public struct ChatSummary: Decodable, Sendable, Equatable {
    public let resource: ChatURI
    public let title: String
    public let status: Int
    public let modifiedAt: String
    public let interactivity: ChatInteractivity
    public var canSend: Bool { interactivity == .full }
}

public struct NativeFeatureMetadata: Decodable, Sendable, Equatable {
    public let textOnly: Bool
    public let permissions: Bool
    public let subagents: Bool
    public let localTools: Bool
    public let sandbox: Bool
    public let attachments: Bool
    public let sendReplay: Bool
    public let liveMultiClient: Bool
    public let reconnect: String
    public let conversationCreation: String
}
struct SnapshotMetadata: Decodable, Sendable, Equatable {
    let native: NativeFeatureMetadata?
    enum CodingKeys: String, CodingKey { case native = "ai.bears.native" }
}

public struct ChatSnapshot: Sendable, Equatable {
    public let resource: ChatURI
    public let fromSeq: UInt64
    public let summary: ChatSummary
    public let turns: [ChatTurn]
    public let activeTurn: ChatTurn?
    public let features: NativeFeatureMetadata?
}
public struct SessionSnapshot: Sendable, Equatable {
    public let resource: SessionURI
    public let fromSeq: UInt64
    public let title: String
    public let status: Int
    public let lifecycle: String
    public let chats: [ChatSummary]
    public let defaultChat: ChatURI
    public let features: NativeFeatureMetadata?
}

enum WireSnapshot: Decodable, Sendable {
    case chat(ChatSnapshot), session(SessionSnapshot), root
    enum CodingKeys: String, CodingKey { case resource, state, fromSeq }
    struct ChatState: Decodable {
        let turns: [ChatTurn]
        let activeTurn: ChatTurn?
        let metadata: SnapshotMetadata?
        enum CodingKeys: String, CodingKey { case turns, activeTurn, metadata = "_meta" }
    }
    struct SessionState: Decodable {
        let title: String
        let status: Int
        let lifecycle: String
        let chats: [ChatSummary]
        let defaultChat: ChatURI
        let metadata: SnapshotMetadata?
        enum CodingKeys: String, CodingKey { case title, status, lifecycle, chats, defaultChat, metadata = "_meta" }
    }
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let channel = try container.decode(AHPChannel.self, forKey: .resource)
        let seq = try container.decode(UInt64.self, forKey: .fromSeq)
        switch channel {
        case .chat(let uri):
            let state = try container.decode(ChatState.self, forKey: .state)
            let summary = try container.decode(ChatSummary.self, forKey: .state)
            guard summary.resource == uri else { throw ClientError.invalidResponse }
            self = .chat(ChatSnapshot(resource: uri, fromSeq: seq, summary: summary, turns: state.turns,
                                      activeTurn: state.activeTurn, features: state.metadata?.native))
        case .session(let uri):
            let state = try container.decode(SessionState.self, forKey: .state)
            self = .session(SessionSnapshot(resource: uri, fromSeq: seq, title: state.title, status: state.status,
                                            lifecycle: state.lifecycle, chats: state.chats, defaultChat: state.defaultChat,
                                            features: state.metadata?.native))
        case .root: self = .root
        }
    }
}

struct InitializeResult: Decodable, Sendable {
    let protocolVersion: String
    let serverSeq: UInt64
    let snapshots: [WireSnapshot]
}
struct SubscribeResult: Decodable, Sendable { let snapshot: WireSnapshot }

struct RPCRequest<Params: Encodable>: Encodable {
    let jsonrpc = "2.0"
    let id: RequestID?
    let method: String
    let params: Params
}
struct InitializeParams: Encodable {
    let channel = AHPChannel.root
    let protocolVersions = ["1.0.0"]
    let clientID: String
    let initialSubscriptions: [AHPChannel]
    enum CodingKeys: String, CodingKey { case channel, protocolVersions, clientID = "clientId", initialSubscriptions }
}
struct SubscriptionParams: Encodable { let channel: AHPChannel }
struct StartParams: Encodable {
    let channel: ChatURI
    let clientSeq: UInt64
    let action: StartAction
}
struct StartAction: Encodable {
    let type = "chat/turnStarted"
    let turnID: TurnID
    let startedAt: String
    let message: UserMessage
    enum CodingKeys: String, CodingKey { case type, turnID = "turnId", startedAt, message }
}
struct CancelParams: Encodable {
    let channel: ChatURI
    let clientSeq: UInt64
    let action: CancelAction
}
struct CancelAction: Encodable {
    let type = "chat/turnCancelled"
    let turnID: TurnID
    let duration: UInt64 = 0
    enum CodingKeys: String, CodingKey { case type, turnID = "turnId", duration }
}
