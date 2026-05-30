import Foundation

protocol AdapterPackageInstalling {
    func installPackage(at packageURL: URL) throws
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

    func installPackage(at packageURL: URL) throws {
        let shellCommand = "/usr/sbin/installer -pkg \(shellQuoted(packageURL.path)) -target /"
        let appleScript = "do shell script \(appleScriptQuoted(shellCommand)) with administrator privileges"

        let result = try processRunner.run(
            URL(fileURLWithPath: "/usr/bin/osascript"),
            arguments: ["-e", appleScript]
        )

        guard result.terminationStatus == 0 else {
            let stderr = result.standardError.trimmingCharacters(in: .whitespacesAndNewlines)
            let stdout = result.standardOutput.trimmingCharacters(in: .whitespacesAndNewlines)
            let message = [stderr, stdout]
                .filter { !$0.isEmpty }
                .joined(separator: "\n")
            throw AdapterPackageInstallerError.installerFailed(
                message.isEmpty
                    ? "macOS installer failed to install the adapter package."
                    : message
            )
        }
    }

    private func shellQuoted(_ string: String) -> String {
        "'" + string.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }

    private func appleScriptQuoted(_ string: String) -> String {
        "\"" + string.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"") + "\""
    }
}
