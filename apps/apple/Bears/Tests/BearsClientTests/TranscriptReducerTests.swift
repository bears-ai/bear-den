import Foundation
import XCTest
@testable import BearsClient

final class TranscriptReducerTests: XCTestCase {
    private func connectedReducer() throws -> TranscriptReducer {
        var reducer = TranscriptReducer(chat: ClientFixtures.chat)
        reducer.apply(.connected(ConnectionID(UUID())))
        reducer.apply(.snapshot(try ClientFixtures.snapshot()))
        return reducer
    }
    private func feed(_ body: String, seq: UInt64, into reducer: inout TranscriptReducer) throws {
        reducer.apply(.action(try ClientFixtures.action(body, seq: seq)))
    }

    func testOrderedMarkdownAndDuplicateDeltaPrevention() throws {
        var reducer = try connectedReducer()
        try feed(ClientFixtures.start, seq: 11, into: &reducer)
        try feed(ClientFixtures.part, seq: 12, into: &reducer)
        try feed(ClientFixtures.delta, seq: 13, into: &reducer)
        try feed(ClientFixtures.delta, seq: 13, into: &reducer)
        try feed(ClientFixtures.delta, seq: 12, into: &reducer)
        XCTAssertEqual(reducer.turns.count, 1)
        XCTAssertEqual(reducer.turns[0].markdown, "**Hello**")
        XCTAssertEqual(reducer.ignoredEventCount, 2)
        XCTAssertEqual(reducer.lastServerSeq, 13)
        try feed("{\"type\":\"chat/turnComplete\",\"turnId\":\"\(ClientFixtures.turn)\",\"duration\":23}", seq: 14, into: &reducer)
        XCTAssertEqual(reducer.turns[0].state, .complete)
        XCTAssertEqual(reducer.turns[0].duration, 23)
        try feed(ClientFixtures.delta, seq: 15, into: &reducer)
        XCTAssertEqual(reducer.turns[0].markdown, "**Hello**")
    }

    func testSnapshotReplacesCanonicalInsteadOfAppendingAndActiveWins() throws {
        var reducer = try connectedReducer()
        try feed(ClientFixtures.start, seq: 11, into: &reducer)
        try feed(ClientFixtures.part, seq: 12, into: &reducer)
        try feed(ClientFixtures.delta, seq: 13, into: &reducer)
        let durable = """
        {"id":"\(ClientFixtures.turn)","startedAt":"2026-10-28T00:00:00Z","message":{"text":"Hello","origin":{"kind":"user"}},
         "responseParts":[{"kind":"markdown","id":"response-1","content":"Canonical"}],"state":"complete"}
        """
        reducer.apply(.snapshot(try ClientFixtures.snapshot(seq: 14, turns: "[\(durable)]")))
        XCTAssertEqual(reducer.turns.count, 1)
        XCTAssertEqual(reducer.turns[0].markdown, "Canonical")
        XCTAssertEqual(reducer.turns[0].state, .complete)
        reducer.apply(.snapshot(try ClientFixtures.snapshot(seq: 12)))
        XCTAssertEqual(reducer.turns.count, 1, "Stale snapshots must not erase newer data")
        let active = """
        {"id":"\(ClientFixtures.turn)","startedAt":"2026-10-28T00:00:00Z","message":{"text":"Hello","origin":{"kind":"user"}},
         "responseParts":[{"kind":"markdown","id":"response-1","content":"Active"}]}
        """
        reducer.apply(.snapshot(try ClientFixtures.snapshot(seq: 15, turns: "[\(durable)]", active: active)))
        XCTAssertEqual(reducer.turns.count, 1)
        XCTAssertEqual(reducer.turns[0].markdown, "Active")
        XCTAssertEqual(reducer.turns[0].state, .streaming)
        reducer.apply(.snapshot(try ClientFixtures.snapshot(seq: 16)))
        XCTAssertTrue(reducer.turns.isEmpty, "Canonical absence replaces stale local state")
    }

    func testRejectionDoesNotCreateTurnAndUnknownEventsAreCounted() throws {
        var reducer = try connectedReducer()
        reducer.apply(.action(try ClientFixtures.action(ClientFixtures.start, seq: 11, rejection: "Not authorized")))
        XCTAssertTrue(reducer.turns.isEmpty)
        XCTAssertEqual(reducer.lastRejectionReason, "Not authorized")
        reducer.apply(.unsupportedEvent(total: 1))
        try feed("{\"type\":\"chat/toolCallStarted\"}", seq: 12, into: &reducer)
        XCTAssertEqual(reducer.unsupportedEventCount, 1)
        XCTAssertTrue(reducer.turns.isEmpty)
        XCTAssertFalse(ClientCapabilities.subagents)
        XCTAssertFalse(ClientCapabilities.permissions)
        XCTAssertFalse(ClientCapabilities.localTools)
        XCTAssertFalse(ClientCapabilities.wasmer)
        XCTAssertFalse(ClientCapabilities.sendReplay)
    }

    func testCancelAndErrorActionsPreservePartialText() throws {
        for type in ["chat/turnCancelled", "chat/error"] {
            var reducer = try connectedReducer()
            try feed(ClientFixtures.start, seq: 11, into: &reducer)
            try feed(ClientFixtures.part, seq: 12, into: &reducer)
            try feed(ClientFixtures.delta, seq: 13, into: &reducer)
            let errorPart = type == "chat/error" ? ",\"part\":{\"kind\":\"error\",\"error\":{\"errorType\":\"turn_failed\",\"message\":\"Failed\"},\"resumable\":false}" : ""
            try feed("{\"type\":\"\(type)\",\"turnId\":\"\(ClientFixtures.turn)\",\"duration\":5\(errorPart)}", seq: 14, into: &reducer)
            XCTAssertEqual(reducer.turns[0].markdown, "**Hello**")
            XCTAssertEqual(reducer.turns[0].state, type == "chat/error" ? .error : .cancelled)
        }
    }

    func testConnectionSequenceResetsAndRequiresNewSnapshot() throws {
        var reducer = try connectedReducer()
        try feed(ClientFixtures.start, seq: 100, into: &reducer)
        reducer.apply(.disconnected)
        XCTAssertTrue(reducer.needsSnapshot)
        XCTAssertFalse(reducer.isConnected)
        reducer.apply(.connected(ConnectionID(UUID())))
        try feed(ClientFixtures.part, seq: 1, into: &reducer)
        XCTAssertTrue(reducer.turns[0].responseParts.isEmpty)
        reducer.apply(.snapshot(try ClientFixtures.snapshot(seq: 2)))
        XCTAssertTrue(reducer.turns.isEmpty)
        try feed(ClientFixtures.start, seq: 3, into: &reducer)
        XCTAssertEqual(reducer.turns.count, 1)
        XCTAssertEqual(reducer.lastServerSeq, 3)
    }

    func testUnknownTurnOrPartCannotInventMessages() throws {
        var reducer = try connectedReducer()
        try feed(ClientFixtures.delta, seq: 11, into: &reducer)
        XCTAssertTrue(reducer.turns.isEmpty)
        try feed(ClientFixtures.start, seq: 12, into: &reducer)
        try feed(ClientFixtures.delta, seq: 13, into: &reducer)
        XCTAssertTrue(reducer.turns[0].responseParts.isEmpty)
        try feed(ClientFixtures.part, seq: 14, into: &reducer)
        try feed(ClientFixtures.part, seq: 15, into: &reducer)
        XCTAssertEqual(reducer.turns[0].responseParts.count, 1)
    }

    func testHistoricalNonUUIDTurnsAndReadOnlySnapshot() throws {
        let historical = """
        {"id":"conversation:42","startedAt":"2026-10-28T00:00:00Z","message":{"text":"Old","origin":{"kind":"user"}},
         "responseParts":[{"kind":"markdown","id":"conversation:43","content":"Reply"}],"state":"complete"}
        """
        let snapshot = try ClientFixtures.snapshot(turns: "[\(historical)]")
        XCTAssertEqual(snapshot.turns[0].id, TurnID("conversation:42"))
        XCTAssertEqual(snapshot.turns[0].markdown, "Reply")
        let data = Data("""
        {"resource":"\(ClientFixtures.chat.rawValue)","fromSeq":1,"state":{"resource":"\(ClientFixtures.chat.rawValue)",
         "title":"Read only","status":1,"modifiedAt":"now","interactivity":"read-only","turns":[]}}
        """.utf8)
        guard case .chat(let readOnly) = try JSONDecoder().decode(WireSnapshot.self, from: data) else { return XCTFail("Expected chat") }
        XCTAssertFalse(readOnly.summary.canSend)
    }
}
