import Foundation

protocol AdapterPackageInstalling {
    func installPackage(at packageURL: URL) throws -> String
}

enum AdapterPackageInstallerError: LocalizedError {
    case installerFailed(String)

    var errorDescription: String? {
        switch self {
        case .installerFailed(let message):
            return message
        }
    }
}

struct InstallerAppAdapterPackageInstaller: AdapterPackageInstalling {
    private let processRunner: ProcessRunning

    init(processRunner: ProcessRunning = FoundationProcessRunner()) {
        self.processRunner = processRunner
    }

    func installPackage(at packageURL: URL) throws -> String {
        let shellCommand = "/usr/sbin/installer -pkg \(shellQuoted(packageURL.path)) -target /"
        let appleScript = "do shell script \(appleScriptQuoted(shellCommand)) with administrator privileges"

        let result = try processRunner.run(
            URL(fileURLWithPath: "/usr/bin/osascript"),
            arguments: ["-e", appleScript]
        )

        let stderr = result.standardError.trimmingCharacters(in: .whitespacesAndNewlines)
        let stdout = result.standardOutput.trimmingCharacters(in: .whitespacesAndNewlines)
        let combinedOutput = [stdout, stderr]
            .filter { !$0.isEmpty }
            .joined(separator: "\n")

        guard result.terminationStatus == 0 else {
            throw AdapterPackageInstallerError.installerFailed(
                combinedOutput.isEmpty
                    ? "macOS installer failed to install the adapter package."
                    : combinedOutput
            )
        }

        return combinedOutput
    }

    private func shellQuoted(_ string: String) -> String {
        "'" + string.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }

    private func appleScriptQuoted(_ string: String) -> String {
        "\"" + string.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"") + "\""
    }
}
