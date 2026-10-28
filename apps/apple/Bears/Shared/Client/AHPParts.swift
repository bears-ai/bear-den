import Foundation

public struct UserMessage: Codable, Sendable, Equatable {
    public struct Origin: Codable, Sendable, Equatable {
        public enum Kind: String, Codable, Sendable { case user }
        public let kind: Kind
        public init() { kind = .user }
    }
    public let text: String
    public let origin: Origin
    public init(text: String) { self.text = text; origin = Origin() }
}

public struct MarkdownPart: Codable, Sendable, Equatable, Identifiable {
    public let id: PartID
    public internal(set) var content: String
    public init(id: PartID, content: String) { self.id = id; self.content = content }
}
public struct TurnErrorInfo: Codable, Sendable, Equatable {
    public let errorType: String
    public let message: String
}
public struct ErrorPart: Codable, Sendable, Equatable {
    public let error: TurnErrorInfo
    public let resumable: Bool
}
public enum ResponsePart: Decodable, Sendable, Equatable {
    case markdown(MarkdownPart)
    case error(ErrorPart)
    case unsupported(kind: String)

    enum CodingKeys: String, CodingKey { case kind }
    public init(from decoder: Decoder) throws {
        let kind = try decoder.container(keyedBy: CodingKeys.self).decode(String.self, forKey: .kind)
        switch kind {
        case "markdown": self = .markdown(try MarkdownPart(from: decoder))
        case "error": self = .error(try ErrorPart(from: decoder))
        default: self = .unsupported(kind: String(kind.prefix(128)))
        }
    }
    public var markdown: String? {
        if case .markdown(let part) = self { return part.content }
        return nil
    }
}

public enum TurnState: String, Codable, Sendable {
    case streaming, complete, cancelled, error
}
public struct ChatTurn: Decodable, Sendable, Equatable, Identifiable {
    public let id: TurnID
    public let startedAt: String
    public let message: UserMessage
    public internal(set) var responseParts: [ResponsePart]
    public internal(set) var state: TurnState
    public internal(set) var duration: UInt64?
    enum CodingKeys: String, CodingKey { case id, startedAt, message, responseParts, state, duration }
    public init(id: TurnID, startedAt: String, message: UserMessage, responseParts: [ResponsePart] = [],
                state: TurnState = .streaming, duration: UInt64? = nil) {
        self.id = id; self.startedAt = startedAt; self.message = message
        self.responseParts = responseParts; self.state = state; self.duration = duration
    }
    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(TurnID.self, forKey: .id)
        startedAt = try container.decode(String.self, forKey: .startedAt)
        message = try container.decode(UserMessage.self, forKey: .message)
        responseParts = try container.decode([ResponsePart].self, forKey: .responseParts)
        state = try container.decodeIfPresent(TurnState.self, forKey: .state) ?? .streaming
        duration = try container.decodeIfPresent(UInt64.self, forKey: .duration)
    }
    public var markdown: String { responseParts.compactMap(\.markdown).joined() }
}
