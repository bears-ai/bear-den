import Foundation
import XCTest
@testable import BearsApp

final class AdapterVersionReaderTests: XCTestCase {
    private let executableURL = URL(fileURLWithPath: "/usr/local/bin/bear-armature")
    private let metadata = #"{"name":"bear-armature","version":"0.1.9","build_git_sha":"abc123","built_at_utc":"2026-10-09T11:00:00Z","local_head_sha":"unavailable","supports_session_list":true,"supports_session_resume":true,"supports_session_load":true,"direct_tools":null,"chrome_tools":"not_probed"}"#
    private let legacyText = """
    bear-armature 0.1.9
    Build git SHA: abc123
    Local HEAD SHA: unknown
    ACP sessions: list/resume/load; conversations bound via Den
    Direct tools: {"fs_read_text_file":{"supported":true}}
    Chrome tools: unavailable
    """

    func testMachineMetadataMatchesArmatureContract() throws {
        let runner = StubProcessRunner(results: [.init(terminationStatus: 0, standardOutput: metadata + "\n", standardError: "")])
        let info = try AdapterVersionReader(processRunner: runner).read(from: executableURL)
        XCTAssertEqual(info.name, "bear-armature")
        XCTAssertEqual(info.version, "0.1.9")
        XCTAssertEqual(info.buildGitSha, "abc123")
        XCTAssertEqual(info.builtAtUtc, "2026-10-09T11:00:00Z")
        XCTAssertTrue(info.supportsSessionList)
        XCTAssertTrue(info.supportsSessionResume)
        XCTAssertTrue(info.supportsSessionLoad)
        XCTAssertNil(info.directTools)
        XCTAssertEqual(info.chromeTools, "not_probed")
        XCTAssertEqual(runner.arguments, [["version", "--json"]])
    }

    func testMinimalLegacyJSONRemainsCompatible() throws {
        let runner = StubProcessRunner(results: [.init(terminationStatus: 0, standardOutput: #"{"version":"0.1.9"}"#, standardError: "")])
        let info = try AdapterVersionReader(processRunner: runner).read(from: executableURL)
        XCTAssertEqual(info.name, "bear-armature")
        XCTAssertEqual(info.version, "0.1.9")
        XCTAssertFalse(info.supportsSessionList)
    }

    func testMachineMetadataOnStderrIsAccepted() throws {
        let runner = StubProcessRunner(results: [.init(terminationStatus: 0, standardOutput: "", standardError: metadata)])
        XCTAssertEqual(try AdapterVersionReader(processRunner: runner).read(from: executableURL).version, "0.1.9")
        XCTAssertEqual(runner.arguments.count, 1)
    }

    func testUnsupportedJSONCommandFallsBackToStderrText() throws {
        let runner = StubProcessRunner(results: [unsupportedJSON, .init(terminationStatus: 0, standardOutput: "", standardError: legacyText)])
        let info = try AdapterVersionReader(processRunner: runner).read(from: executableURL)
        XCTAssertEqual(info.version, "0.1.9")
        XCTAssertEqual(info.buildGitSha, "abc123")
        XCTAssertEqual(info.directTools?["fs_read_text_file"], .object(["supported": .bool(true)]))
        XCTAssertEqual(runner.arguments, [["version", "--json"], ["--version"]])
    }

    func testStdoutDiagnosticsDoNotMaskStderrMetadata() throws {
        let runner = StubProcessRunner(results: [unsupportedJSON, .init(terminationStatus: 0, standardOutput: "warning: unrelated diagnostic", standardError: legacyText)])
        XCTAssertEqual(try AdapterVersionReader(processRunner: runner).read(from: executableURL).version, "0.1.9")
    }

    func testTextBannerAndLegacyExecutableNameAreAccepted() throws {
        let text = "warning: legacy build\n" + legacyText.replacingOccurrences(of: "bear-armature", with: "bears-acp-adapter")
        let runner = StubProcessRunner(results: [unsupportedJSON, .init(terminationStatus: 0, standardOutput: text, standardError: "")])
        let info = try AdapterVersionReader(processRunner: runner).read(from: executableURL)
        XCTAssertEqual(info.name, "bears-acp-adapter")
        XCTAssertEqual(info.version, "0.1.9")
    }

    func testInvalidJSONVersionDoesNotSuppressTextFallback() throws {
        let invalid = metadata.replacingOccurrences(of: "0.1.9", with: "")
        let runner = StubProcessRunner(results: [.init(terminationStatus: 0, standardOutput: invalid, standardError: ""), .init(terminationStatus: 0, standardOutput: legacyText, standardError: "")])
        XCTAssertEqual(try AdapterVersionReader(processRunner: runner).read(from: executableURL).version, "0.1.9")
        XCTAssertEqual(runner.arguments.count, 2)
    }

    func testDiagnosticTextIsNotMisclassifiedAsVersionMetadata() {
        let runner = StubProcessRunner(results: [unsupportedJSON, .init(terminationStatus: 0, standardOutput: "", standardError: "dyld: Library not loaded")])
        XCTAssertThrowsError(try AdapterVersionReader(processRunner: runner).read(from: executableURL)) { error in
            XCTAssertTrue(error.localizedDescription.contains("No recognized armature version metadata"))
            XCTAssertTrue(error.localizedDescription.contains(self.executableURL.path))
            XCTAssertTrue(error.localizedDescription.contains("dyld: Library not loaded"))
        }
    }

    func testNonzeroVersionExitCannotBeAcceptedDespiteVersionLookingOutput() {
        let runner = StubProcessRunner(results: [unsupportedJSON, .init(terminationStatus: 1, standardOutput: legacyText, standardError: "cannot execute installed binary")])
        XCTAssertThrowsError(try AdapterVersionReader(processRunner: runner).read(from: executableURL)) { error in
            XCTAssertTrue(error.localizedDescription.contains("--version exited with status 1"))
            XCTAssertTrue(error.localizedDescription.contains("cannot execute installed binary"))
            XCTAssertTrue(error.localizedDescription.contains("version --json (exit 1)"))
        }
    }

    func testVerboseStdoutCannotHideStderrFailure() {
        let runner = StubProcessRunner(results: [unsupportedJSON, .init(terminationStatus: 1, standardOutput: String(repeating: "x", count: 10_000), standardError: "Permission denied")])
        XCTAssertThrowsError(try AdapterVersionReader(processRunner: runner).read(from: executableURL)) { error in
            XCTAssertTrue(error.localizedDescription.contains("stderr: Permission denied"))
            XCTAssertLessThan(error.localizedDescription.count, 5000)
        }
    }

    func testLaunchFailurePreservesItsActualReason() {
        let runner = StubProcessRunner(results: [], launchError: NSError(domain: NSPOSIXErrorDomain, code: 13, userInfo: [NSLocalizedDescriptionKey: "Permission denied"]))
        XCTAssertThrowsError(try AdapterVersionReader(processRunner: runner).read(from: executableURL)) { error in
            XCTAssertEqual(error.localizedDescription, "Permission denied")
        }
        XCTAssertEqual(runner.arguments, [["version", "--json"]])
    }

    private var unsupportedJSON: ProcessResult {
        .init(terminationStatus: 1, standardOutput: "", standardError: "unknown subcommand version")
    }
}

private final class StubProcessRunner: ProcessRunning {
    private var results: [ProcessResult]
    private let launchError: Error?
    private(set) var arguments: [[String]] = []

    init(results: [ProcessResult], launchError: Error? = nil) {
        self.results = results
        self.launchError = launchError
    }

    func run(_ executableURL: URL, arguments: [String]) throws -> ProcessResult {
        self.arguments.append(arguments)
        if let launchError { throw launchError }
        guard !results.isEmpty else {
            throw NSError(domain: "Bears.TestProcessRunner", code: 1, userInfo: [NSLocalizedDescriptionKey: "Unexpected version probe"])
        }
        return results.removeFirst()
    }
}
