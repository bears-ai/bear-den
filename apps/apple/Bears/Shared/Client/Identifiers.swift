import Foundation

/// Distinct wire identifiers retain their native single-value JSON representation.
public struct ClientID<Tag, Value: Hashable & Codable & Sendable>: RawRepresentable, Hashable, Codable, Sendable {
    public let rawValue: Value
    public init(rawValue: Value) { self.rawValue = rawValue }
    public init(_ value: Value) { self.rawValue = value }
    public init(from decoder: Decoder) throws { rawValue = try decoder.singleValueContainer().decode(Value.self) }
    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(rawValue)
    }
}

public enum BearTag: Sendable {}
public enum HatTag: Sendable {}
public enum ConversationTag: Sendable {}
public enum MessageTag: Sendable {}
public enum TurnTag: Sendable {}
public enum PartTag: Sendable {}
public enum RequestTag: Sendable {}
public enum ConnectionTag: Sendable {}
public enum SubjectTag: Sendable {}
public typealias BearID = ClientID<BearTag, UUID>
public typealias HatID = ClientID<HatTag, UUID>
public typealias ConversationID = ClientID<ConversationTag, String>
public typealias MessageID = ClientID<MessageTag, String>
// Historical snapshots can use conversation:sequence IDs rather than UUIDs.
public typealias TurnID = ClientID<TurnTag, String>
public typealias PartID = ClientID<PartTag, String>
public typealias RequestID = ClientID<RequestTag, UUID>
public typealias ConnectionID = ClientID<ConnectionTag, UUID>
public typealias SubjectID = ClientID<SubjectTag, String>

public protocol AHPResourceKind: Sendable { static var scheme: String { get } }
public enum ChatResource: AHPResourceKind { public static let scheme = "ahp-chat" }
public enum SessionResource: AHPResourceKind { public static let scheme = "ahp-session" }

public struct AHPResource<Kind: AHPResourceKind>: Hashable, Codable, Sendable {
    public let conversationUUID: UUID
    public var rawValue: String { "\(Kind.scheme):/\(conversationUUID.uuidString.lowercased())" }

    public init(_ uuid: UUID) throws {
        guard uuid != UUID(uuid: (0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)) else {
            throw ClientError.invalidInput
        }
        conversationUUID = uuid
    }
    public init(rawValue: String) throws {
        let prefix = Kind.scheme + ":/"
        guard rawValue.hasPrefix(prefix), let uuid = UUID(uuidString: String(rawValue.dropFirst(prefix.count))) else {
            throw ClientError.invalidInput
        }
        try self.init(uuid)
        guard self.rawValue == rawValue else { throw ClientError.invalidInput }
    }
    public init(from decoder: Decoder) throws {
        try self.init(rawValue: decoder.singleValueContainer().decode(String.self))
    }
    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(rawValue)
    }
}
public typealias ChatURI = AHPResource<ChatResource>
public typealias SessionURI = AHPResource<SessionResource>

public enum AHPChannel: Hashable, Codable, Sendable {
    case root
    case session(SessionURI)
    case chat(ChatURI)

    public var rawValue: String {
        switch self {
        case .root: return "ahp-root://"
        case .session(let uri): return uri.rawValue
        case .chat(let uri): return uri.rawValue
        }
    }
    public init(rawValue: String) throws {
        if rawValue == "ahp-root://" { self = .root }
        else if let chat = try? ChatURI(rawValue: rawValue) { self = .chat(chat) }
        else if let session = try? SessionURI(rawValue: rawValue) { self = .session(session) }
        else { throw ClientError.invalidInput }
    }
    public init(from decoder: Decoder) throws {
        try self.init(rawValue: decoder.singleValueContainer().decode(String.self))
    }
    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(rawValue)
    }
}
