import Foundation

public struct TurnStarted: Decodable, Sendable, Equatable {
    public let turnID: TurnID
    public let startedAt: String
    public let message: UserMessage
    enum CodingKeys: String, CodingKey { case turnID = "turnId", startedAt, message }
}
public struct ResponsePartAction: Decodable, Sendable, Equatable {
    public let turnID: TurnID
    public let part: ResponsePart
    enum CodingKeys: String, CodingKey { case turnID = "turnId", part }
}
public struct TextDelta: Decodable, Sendable, Equatable {
    public let turnID: TurnID
    public let partID: PartID
    public let content: String
    enum CodingKeys: String, CodingKey { case turnID = "turnId", partID = "partId", content }
}
public struct TurnEnd: Decodable, Sendable, Equatable {
    public let turnID: TurnID
    public let duration: UInt64
    enum CodingKeys: String, CodingKey { case turnID = "turnId", duration }
}
public struct TurnFailure: Decodable, Sendable, Equatable {
    public let turnID: TurnID
    public let duration: UInt64
    public let part: ResponsePart
    enum CodingKeys: String, CodingKey { case turnID = "turnId", duration, part }
}

public enum AHPAction: Decodable, Sendable, Equatable {
    case turnStarted(TurnStarted)
    case responsePart(ResponsePartAction)
    case delta(TextDelta)
    case turnComplete(TurnEnd)
    case cancelled(TurnEnd)
    case error(TurnFailure)
    case unsupported(type: String)

    enum CodingKeys: String, CodingKey { case type }
    public init(from decoder: Decoder) throws {
        let type = try decoder.container(keyedBy: CodingKeys.self).decode(String.self, forKey: .type)
        switch type {
        case "chat/turnStarted": self = .turnStarted(try TurnStarted(from: decoder))
        case "chat/responsePart": self = .responsePart(try ResponsePartAction(from: decoder))
        case "chat/delta": self = .delta(try TextDelta(from: decoder))
        case "chat/turnComplete": self = .turnComplete(try TurnEnd(from: decoder))
        case "chat/turnCancelled": self = .cancelled(try TurnEnd(from: decoder))
        case "chat/error": self = .error(try TurnFailure(from: decoder))
        default: self = .unsupported(type: String(type.prefix(128)))
        }
    }
    public var turnID: TurnID? {
        switch self {
        case .turnStarted(let value): return value.turnID
        case .responsePart(let value): return value.turnID
        case .delta(let value): return value.turnID
        case .turnComplete(let value), .cancelled(let value): return value.turnID
        case .error(let value): return value.turnID
        case .unsupported: return nil
        }
    }
    public var isUnsupported: Bool {
        switch self {
        case .unsupported: return true
        case .responsePart(let value): if case .unsupported = value.part { return true }; return false
        case .error(let value): if case .unsupported = value.part { return true }; return false
        default: return false
        }
    }
}

public struct ActionOrigin: Decodable, Sendable, Equatable {
    public let clientID: String
    public let clientSeq: UInt64
    enum CodingKeys: String, CodingKey { case clientID = "clientId", clientSeq }
}
public struct AHPActionEnvelope: Decodable, Sendable, Equatable {
    public let channel: AHPChannel
    public let action: AHPAction
    public let serverSeq: UInt64
    public let origin: ActionOrigin?
    public let rejectionReason: String?
}

public struct DispatchReceipt: Sendable, Equatable {
    public let connectionID: ConnectionID
    public let clientSeq: UInt64
    public let turnID: TurnID
}

public enum ClientEvent: Sendable, Equatable {
    case connected(ConnectionID)
    case initialized(protocolVersion: String)
    case snapshot(ChatSnapshot)
    case sessionSnapshot(SessionSnapshot)
    case action(AHPActionEnvelope)
    case unsupportedEvent(total: UInt64)
    case disconnected
}

/// This text channel is not an ACP armature and exposes no local execution APIs.
public enum ClientCapabilities {
    public static let textOnly = true
    public static let subagents = false
    public static let permissions = false
    public static let localTools = false
    public static let wasmer = false
    public static let sandbox = false
    public static let attachments = false
    public static let sendReplay = false
    public static let liveMultiClient = false
}
