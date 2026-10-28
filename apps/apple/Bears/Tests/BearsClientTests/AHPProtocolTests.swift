import Foundation
import XCTest
@testable import BearsClient

final class AHPProtocolTests: XCTestCase {
    private func initialize(_ state: inout AHPProtocolState, seq: UInt64 = 10) throws {
        let id = RequestID(UUID())
        state.register(id, purpose: .initialize)
        let result = try state.process(Data("""
        {"jsonrpc":"2.0","id":"\(id.rawValue)","result":{"protocolVersion":"1.0.0","serverSeq":\(seq),"snapshots":[]}}
        """.utf8))
        XCTAssertEqual(result.completedRequestID, id)
        XCTAssertEqual(result.events, [.initialized(protocolVersion: "1.0.0")])
    }
    private func notification(_ action: String, seq: UInt64, channel: String? = nil) -> Data {
        Data("""
        {"jsonrpc":"2.0","method":"action","params":{"channel":"\(channel ?? ClientFixtures.chat.rawValue)","serverSeq":\(seq),"action":\(action)}}
        """.utf8)
    }

    func testRequestCorrelationAndNoGuessedInitialization() throws {
        var state = AHPProtocolState()
        let expected = RequestID(UUID())
        state.register(expected, purpose: .initialize)
        let unknown = try state.process(Data("""
        {"jsonrpc":"2.0","id":"\(UUID())","result":{"protocolVersion":"1.0.0","serverSeq":0,"snapshots":[]}}
        """.utf8))
        XCTAssertNil(unknown.completedRequestID)
        XCTAssertEqual(unknown.events, [.unsupportedEvent(total: 1)])
        XCTAssertFalse(state.initialized)
        let accepted = try state.process(Data("""
        {"jsonrpc":"2.0","id":"\(expected.rawValue)","result":{"protocolVersion":"1.0.0","serverSeq":0,"snapshots":[]}}
        """.utf8))
        XCTAssertEqual(accepted.completedRequestID, expected)
        XCTAssertTrue(state.initialized)
        var uninitialized = AHPProtocolState()
        XCTAssertThrowsError(try uninitialized.process(notification(ClientFixtures.start, seq: 1)))
    }

    func testServerSequenceIsMonotonicButNotRequiredToBeContiguous() throws {
        var state = AHPProtocolState()
        try initialize(&state)
        let start = try state.process(notification(ClientFixtures.start, seq: 20))
        XCTAssertEqual(start.events.count, 1)
        guard case .action(let envelope) = start.events[0] else { return XCTFail("Expected action") }
        XCTAssertEqual(envelope.serverSeq, 20)
        XCTAssertEqual(envelope.action.turnID, TurnID(ClientFixtures.turn))
        XCTAssertTrue(try state.process(notification(ClientFixtures.start, seq: 20)).events.isEmpty)
        XCTAssertTrue(try state.process(notification(ClientFixtures.start, seq: 19)).events.isEmpty)
        XCTAssertEqual(state.duplicateEventCount, 2)
        XCTAssertEqual(try state.process(notification(ClientFixtures.part, seq: 25)).events.count, 1)
    }

    func testUnsupportedEventsAndPartsAreCountedWithoutInventingAPIs() throws {
        var state = AHPProtocolState()
        try initialize(&state)
        let summary = try state.process(Data("{\"jsonrpc\":\"2.0\",\"method\":\"root/sessionSummaryChanged\",\"params\":{}}".utf8))
        XCTAssertEqual(summary.events, [.unsupportedEvent(total: 1)])
        let tool = try state.process(notification("{\"type\":\"chat/toolCallStarted\"}", seq: 11))
        XCTAssertEqual(tool.events.first, .unsupportedEvent(total: 2))
        guard case .action(let envelope) = tool.events.last else { return XCTFail("Expected retained unsupported envelope") }
        XCTAssertEqual(envelope.action, .unsupported(type: "chat/toolCallStarted"))
        let part = """
        {"type":"chat/responsePart","turnId":"\(ClientFixtures.turn)","part":{"kind":"toolCall","id":"tool"}}
        """
        XCTAssertEqual(try state.process(notification(part, seq: 12)).events.first, .unsupportedEvent(total: 3))
        let session = try state.process(notification("{\"type\":\"session/chatUpdated\",\"changes\":{}}", seq: 13, channel: ClientFixtures.session.rawValue))
        XCTAssertEqual(session.events.first, .unsupportedEvent(total: 4))
        XCTAssertEqual(state.unsupportedEventCount, 4)
    }

    func testSubscribeSnapshotActualBackendShapeAndResourceCorrelation() throws {
        var state = AHPProtocolState()
        try initialize(&state)
        let id = RequestID(UUID())
        state.register(id, purpose: .subscribe(.chat(ClientFixtures.chat)))
        XCTAssertTrue(state.reservedSubscriptions.contains(.chat(ClientFixtures.chat)))
        let frame = try state.process(Data("""
        {"jsonrpc":"2.0","id":"\(id.rawValue)","result":{"snapshot":{
          "resource":"\(ClientFixtures.chat.rawValue)","fromSeq":15,"state":{
          "resource":"\(ClientFixtures.chat.rawValue)","title":"Chat","status":8,"modifiedAt":"2026-10-28T00:00:00Z",
          "interactivity":"full","turns":[],"activeTurn":{"id":"\(ClientFixtures.turn)","startedAt":"2026-10-28T00:00:00Z",
          "message":{"text":"Hi","origin":{"kind":"user"}},"responseParts":[]},
          "_meta":{"ai.bears.native":{"textOnly":true,"permissions":false,"subagents":false,"localTools":false,"sandbox":false,
          "attachments":false,"sendReplay":false,"liveMultiClient":false,"reconnect":"snapshot","conversationCreation":"/v1.0/chat/conversations"}}}}}}
        """.utf8))
        XCTAssertEqual(frame.completedRequestID, id)
        guard case .snapshot(let snapshot) = frame.events.first else { return XCTFail("Expected snapshot") }
        XCTAssertEqual(snapshot.activeTurn?.state, .streaming)
        XCTAssertEqual(snapshot.features?.permissions, false)
        XCTAssertTrue(state.subscriptions.contains(.chat(ClientFixtures.chat)))
        XCTAssertTrue(try state.process(notification(ClientFixtures.delta, seq: 15)).events.isEmpty)

        let badID = RequestID(UUID())
        state.register(badID, purpose: .subscribe(.session(ClientFixtures.session)))
        XCTAssertThrowsError(try state.process(Data("""
        {"jsonrpc":"2.0","id":"\(badID.rawValue)","result":{"snapshot":{"resource":"ahp-root://","fromSeq":16,"state":{}}}}
        """.utf8))) { XCTAssertEqual($0 as? ClientError, .invalidResponse) }
    }

    func testSessionSnapshotAndInitializationRequireAllRequestedResources() throws {
        let sessionJSON = """
        {"resource":"\(ClientFixtures.session.rawValue)","fromSeq":5,"state":{"provider":"den","title":"Chat","status":1,
         "lifecycle":"ready","activeClients":[],"chats":[],"defaultChat":"\(ClientFixtures.chat.rawValue)"}}
        """
        guard case .session(let snapshot) = try JSONDecoder().decode(WireSnapshot.self, from: Data(sessionJSON.utf8)) else {
            return XCTFail("Expected session")
        }
        XCTAssertEqual(snapshot.defaultChat, ClientFixtures.chat)
        XCTAssertEqual(snapshot.lifecycle, "ready")
        var state = AHPProtocolState(subscriptions: [.session(ClientFixtures.session)])
        let id = RequestID(UUID())
        state.register(id, purpose: .initialize)
        let frame = try state.process(Data("""
        {"jsonrpc":"2.0","id":"\(id.rawValue)","result":{"protocolVersion":"1.0.0","serverSeq":5,"snapshots":[\(sessionJSON)]}}
        """.utf8))
        XCTAssertEqual(frame.events, [.sessionSnapshot(snapshot), .initialized(protocolVersion: "1.0.0")])
        var missing = AHPProtocolState(subscriptions: [.chat(ClientFixtures.chat)])
        missing.register(id, purpose: .initialize)
        XCTAssertThrowsError(try missing.process(Data("""
        {"jsonrpc":"2.0","id":"\(id.rawValue)","result":{"protocolVersion":"1.0.0","serverSeq":5,"snapshots":[]}}
        """.utf8)))
    }

    func testRejectionReasonAndSafeRPCErrors() throws {
        var state = AHPProtocolState()
        try initialize(&state)
        let envelope = try ClientFixtures.action(ClientFixtures.start, seq: 11, rejection: "Do not replay sends")
        let data = Data("""
        {"jsonrpc":"2.0","method":"action","params":{"channel":"\(ClientFixtures.chat.rawValue)","serverSeq":11,
         "origin":{"clientId":"apple-test","clientSeq":1},"rejectionReason":"Do not replay sends","action":\(ClientFixtures.start)}}
        """.utf8)
        XCTAssertEqual(try state.process(data).events, [.action(envelope)])
        XCTAssertThrowsError(try state.process(Data("{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32602,\"message\":\"secret\"}}".utf8))) {
            XCTAssertEqual($0 as? ClientError, .rpc(code: -32602))
            XCTAssertFalse(String(reflecting: $0).contains("secret"))
        }
        XCTAssertThrowsError(try state.process(Data("{\"jsonrpc\":\"1.0\"}".utf8)))
        XCTAssertThrowsError(try state.process(Data("not JSON secret".utf8)))
    }

    func testOversizedFramesFailClosed() {
        var state = AHPProtocolState()
        XCTAssertThrowsError(try state.process(Data(repeating: 65, count: 2 * 1024 * 1024 + 1))) {
            XCTAssertEqual($0 as? ClientError, .responseTooLarge)
        }
    }

    func testOutboundReleasedWireShapes() throws {
        let initialize = RPCRequest(id: RequestID(ClientFixtures.uuid), method: "initialize", params:
            InitializeParams(clientID: "apple", initialSubscriptions: [.chat(ClientFixtures.chat), .session(ClientFixtures.session)]))
        let json = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(initialize)) as? [String: Any])
        let params = try XCTUnwrap(json["params"] as? [String: Any])
        XCTAssertEqual(params["protocolVersions"] as? [String], ["1.0.0"])
        XCTAssertEqual(params["clientId"] as? String, "apple")
        XCTAssertEqual(params["channel"] as? String, "ahp-root://")
        let start = RPCRequest(id: nil, method: "dispatchAction", params: StartParams(channel: ClientFixtures.chat, clientSeq: 1,
            action: StartAction(turnID: TurnID(ClientFixtures.turn), startedAt: "2026-10-28T00:00:00Z", message: UserMessage(text: "Hi"))))
        let startJSON = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(start)) as? [String: Any])
        XCTAssertNil(startJSON["id"])
        let startParams = try XCTUnwrap(startJSON["params"] as? [String: Any])
        XCTAssertEqual(startParams["clientSeq"] as? Int, 1)
        let action = try XCTUnwrap(startParams["action"] as? [String: Any])
        XCTAssertEqual(Set(action.keys), ["type", "turnId", "startedAt", "message"])
        XCTAssertEqual(action["type"] as? String, "chat/turnStarted")
    }

    func testDisconnectedClientRejectsSendAndCancelWithoutConnecting() async throws {
        let client = AHPClient(configuration: try ClientFixtures.configuration())
        do { _ = try await client.send(text: "Hi", to: ClientFixtures.chat); XCTFail("No socket") }
        catch { XCTAssertEqual(error as? ClientError, .notInitialized) }
        do { _ = try await client.cancel(turnID: TurnID(ClientFixtures.turn), in: ClientFixtures.chat); XCTFail("No socket") }
        catch { XCTAssertEqual(error as? ClientError, .notInitialized) }
        await client.disconnect()
        await client.disconnect()
    }
}

