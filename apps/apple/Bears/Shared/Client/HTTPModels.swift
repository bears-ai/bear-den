import Foundation

public struct BearSummary: Codable, Sendable, Equatable, Identifiable {
    public var id: BearID { bearID }
    public let bearID: BearID
    public let slug: String
    public let name: String
    public let description: String
    public let isBearAdmin: Bool
    enum CodingKeys: String, CodingKey {
        case bearID = "bear_id", slug, name, description, isBearAdmin = "is_bear_admin"
    }
}

public struct ConversationSummary: Codable, Sendable, Equatable, Identifiable {
    public let id: ConversationID
    public let title: String
    public let hatID: HatID?
    public let canSend: Bool
    public let lastMessageAt: String?
    public let sessionURI: SessionURI
    public let chatURI: ChatURI
    enum CodingKeys: String, CodingKey {
        case id, title, hatID = "hat_id", canSend = "can_send", lastMessageAt = "last_message_at"
        case sessionURI = "session_uri", chatURI = "chat_uri"
    }
}

public struct HatSummary: Codable, Sendable, Equatable, Identifiable {
    public let id: HatID
    public let name: String
    public let shortSummary: String?
    enum CodingKeys: String, CodingKey { case id, name, shortSummary = "short_summary" }
}

public struct ConversationInventory: Codable, Sendable, Equatable {
    public let conversations: [ConversationSummary]
    public let hats: [HatSummary]
}

public struct CreateConversation: Codable, Sendable, Equatable {
    public let bearID: BearID
    public let hatID: HatID
    public let requestID: RequestID
    public init(bearID: BearID, hatID: HatID, requestID: RequestID = RequestID(UUID())) {
        self.bearID = bearID
        self.hatID = hatID
        self.requestID = requestID
    }
    enum CodingKeys: String, CodingKey { case bearID = "bear_id", hatID = "hat_id", requestID = "request_id" }
}

public struct CreatedConversation: Codable, Sendable, Equatable, Identifiable {
    public let id: ConversationID
    public let title: String
    public let hatID: HatID
    public let sessionURI: SessionURI
    public let chatURI: ChatURI
    enum CodingKeys: String, CodingKey {
        case id, title, hatID = "hat_id", sessionURI = "session_uri", chatURI = "chat_uri"
    }
}

/// Unknown role/kind values remain explicit wire data rather than being presented as assistant messages.
public enum HistoryRole: Equatable, Sendable, Codable {
    case user, assistant, system, tool, unsupported(String)
    public init(from decoder: Decoder) throws {
        let value = try decoder.singleValueContainer().decode(String.self)
        switch value {
        case "user": self = .user
        case "assistant": self = .assistant
        case "system": self = .system
        case "tool": self = .tool
        default: self = .unsupported(value)
        }
    }
    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .user: try container.encode("user")
        case .assistant: try container.encode("assistant")
        case .system: try container.encode("system")
        case .tool: try container.encode("tool")
        case .unsupported(let value): try container.encode(value)
        }
    }
}
public struct HistoryMessage: Codable, Sendable, Equatable, Identifiable {
    public let id: MessageID
    public let sequenceNo: Int64
    public let role: HistoryRole
    public let kind: String
    public let text: String
    public let createdAt: String
    enum CodingKeys: String, CodingKey { case id, sequenceNo = "sequence_no", role, kind, text, createdAt = "created_at" }
}

public struct HistoryPage: Codable, Sendable, Equatable {
    public let messages: [HistoryMessage]
    public let hasMore: Bool
    /// Den sends this numeric cursor as a JSON string.
    public let nextBefore: String?
    public var beforeSequence: Int64? { nextBefore.flatMap(Int64.init) }
    enum CodingKeys: String, CodingKey { case messages, hasMore = "has_more", nextBefore = "next_before" }
}

public struct UserInfo: Codable, Sendable, Equatable {
    public let subject: SubjectID
    public let preferredUsername: String?
    public let name: String?
    public let email: String?
    enum CodingKeys: String, CodingKey { case subject = "sub", preferredUsername = "preferred_username", name, email }
}
