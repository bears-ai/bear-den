import Foundation

protocol ProcessRunning {
    @discardableResult
    func run(_ executableURL: URL, arguments: [String]) throws -> ProcessResult
}

struct ProcessResult: Equatable {
    let terminationStatus: Int32
    let standardOutput: String
    let standardError: String
}

struct FoundationProcessRunner: ProcessRunning {
    @discardableResult
    func run(_ executableURL: URL, arguments: [String]) throws -> ProcessResult {
        let process = Process()
        process.executableURL = executableURL
        process.arguments = arguments

        let stdout = Pipe()
        let stderr = Pipe()
        process.standardOutput = stdout
        process.standardError = stderr

        try process.run()
        process.waitUntilExit()

        let standardOutput = String(
            data: stdout.fileHandleForReading.readDataToEndOfFile(),
            encoding: .utf8
        ) ?? ""
        let standardError = String(
            data: stderr.fileHandleForReading.readDataToEndOfFile(),
            encoding: .utf8
        ) ?? ""

        return ProcessResult(
            terminationStatus: process.terminationStatus,
            standardOutput: standardOutput,
            standardError: standardError
        )
    }
}
