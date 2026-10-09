import Foundation

/// Reads CLI metadata without mistaking diagnostics for a successful version probe.
struct AdapterVersionReader {
    let processRunner: ProcessRunning

    func read(from executableURL: URL) throws -> AdapterVersionInfo {
        let jsonResult = try processRunner.run(executableURL, arguments: ["version", "--json"])
        if jsonResult.terminationStatus == 0 {
            for output in [jsonResult.standardOutput, jsonResult.standardError] {
                if let info = decodeJSON(output) {
                    return info
                }
            }
        }

        // Older armatures report their human-readable version on stderr.
        let textResult = try processRunner.run(executableURL, arguments: ["--version"])
        if textResult.terminationStatus == 0 {
            for output in [textResult.standardOutput, textResult.standardError] {
                if let info = parseText(output) {
                    return info
                }
            }
        }

        throw VersionProbeError(executableURL: executableURL, jsonResult: jsonResult, textResult: textResult)
    }

    private func decodeJSON(_ output: String) -> AdapterVersionInfo? {
        let output = output.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let start = output.firstIndex(of: "{") else { return nil }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        guard let info = try? decoder.decode(AdapterVersionInfo.self, from: Data(output[start...].utf8)),
              isArmatureName(info.name), isVersion(info.version) else {
            return nil
        }
        return info
    }

    private func parseText(_ output: String) -> AdapterVersionInfo? {
        let lines = output.split(separator: "\n")
            .map { String($0).trimmingCharacters(in: .whitespacesAndNewlines) }
        guard let headerIndex = lines.firstIndex(where: { line in
            let parts = line.split(whereSeparator: { $0.isWhitespace })
            return parts.count == 2 && isArmatureName(String(parts[0])) && isVersion(String(parts[1]))
        }) else {
            return nil
        }
        let header = lines[headerIndex].split(whereSeparator: { $0.isWhitespace })
        var fields: [String: String] = [:]
        for line in lines.dropFirst(headerIndex + 1) {
            let parts = line.split(separator: ":", maxSplits: 1).map(String.init)
            guard parts.count == 2 else { continue }
            fields[parts[0].trimmingCharacters(in: .whitespacesAndNewlines)] = parts[1].trimmingCharacters(in: .whitespacesAndNewlines)
        }
        let directTools = fields["Direct tools"].flatMap { raw in
            try? JSONDecoder().decode([String: JSONValue].self, from: Data(raw.utf8))
        }
        return AdapterVersionInfo(
            name: String(header[0]),
            version: String(header[1]),
            buildGitSha: fields["Build git SHA"] ?? "unknown",
            builtAtUtc: fields["Built at UTC"] ?? "unknown",
            localHeadSha: fields["Local HEAD SHA"] ?? "unknown",
            supportsSessionList: fields["ACP sessions"]?.contains("list") ?? false,
            supportsSessionResume: fields["ACP sessions"]?.contains("resume") ?? false,
            supportsSessionLoad: fields["ACP sessions"]?.contains("load") ?? false,
            directTools: directTools,
            chromeTools: fields["Chrome tools"] ?? "unknown"
        )
    }

    private func isArmatureName(_ name: String) -> Bool {
        name == "bear-armature" || name == "bears-acp-adapter"
    }

    private func isVersion(_ version: String) -> Bool {
        guard version == version.trimmingCharacters(in: .whitespacesAndNewlines) else { return false }
        // Preserve older two-component releases, but never accept arbitrary diagnostic text.
        return version.range(
            of: #"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:\.(0|[1-9][0-9]*))?(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$"#,
            options: .regularExpression
        ) != nil
    }
}

private struct VersionProbeError: LocalizedError {
    let executableURL: URL
    let jsonResult: ProcessResult
    let textResult: ProcessResult

    var errorDescription: String? {
        let reason = textResult.terminationStatus == 0
            ? "No recognized armature version metadata was returned."
            : "Armature --version exited with status \(textResult.terminationStatus)."
        return (["\(reason) Executable: \(executableURL.path)"] + [
            details(for: "version --json", result: jsonResult),
            details(for: "--version", result: textResult)
        ]).joined(separator: "\n")
    }

    private func details(for command: String, result: ProcessResult) -> String {
        let stdout = result.standardOutput.trimmingCharacters(in: .whitespacesAndNewlines)
        let stderr = result.standardError.trimmingCharacters(in: .whitespacesAndNewlines)
        return "\(command) (exit \(result.terminationStatus)):\nstdout: \(bounded(stdout))\nstderr: \(bounded(stderr))"
    }

    private func bounded(_ output: String) -> String {
        output.isEmpty ? "(no output)" : String(output.prefix(2048))
    }
}
