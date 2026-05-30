import Foundation

struct AdapterInstallManager: AdapterInstallManaging, AdapterVersionProviding {
    private let pathProvider: BearsPathResolver
    private let bundledAdapterLocator: BundledAdapterLocating
    private let artifactSourceProvider: AdapterArtifactSourceProviding
    private let gitHubReleaseAdapterSource: GitHubReleaseAdapterSource
    private let artifactDownloader: AdapterArtifactDownloading
    private let packageInstaller: AdapterPackageInstalling
    private let processRunner: ProcessRunning
    private let fileManager: FileManager
    private let jsonDecoder: JSONDecoder
    private let jsonEncoder: JSONEncoder

    init(
        pathProvider: BearsPathResolver = BearsPathResolver(),
        bundledAdapterLocator: BundledAdapterLocating = BundledAdapterLocator(),
        artifactSourceProvider: AdapterArtifactSourceProviding = GitHubReleaseAdapterSource(),
        gitHubReleaseAdapterSource: GitHubReleaseAdapterSource = GitHubReleaseAdapterSource(),
        artifactDownloader: AdapterArtifactDownloading = URLSessionAdapterArtifactDownloader(),
        packageInstaller: AdapterPackageInstalling = InstallerAppAdapterPackageInstaller(),
        processRunner: ProcessRunning = FoundationProcessRunner(),
        fileManager: FileManager = .default
    ) {
        self.pathProvider = pathProvider
        self.bundledAdapterLocator = bundledAdapterLocator
        self.artifactSourceProvider = artifactSourceProvider
        self.gitHubReleaseAdapterSource = gitHubReleaseAdapterSource
        self.artifactDownloader = artifactDownloader
        self.packageInstaller = packageInstaller
        self.processRunner = processRunner
        self.fileManager = fileManager

        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        self.jsonDecoder = decoder

        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        encoder.dateEncodingStrategy = .iso8601
        encoder.keyEncodingStrategy = .convertToSnakeCase
        self.jsonEncoder = encoder
    }

    func loadInstallState() throws -> InstallState? {
        guard fileManager.fileExists(atPath: pathProvider.installStatePath.path) else {
            return nil
        }

        let data = try Data(contentsOf: pathProvider.installStatePath)
        return try jsonDecoder.decode(InstallState.self, from: data)
    }

    func inspectInstallState() throws -> InstallState {
        let managedAdapterExists = fileManager.fileExists(atPath: pathProvider.managedAdapterPath.path)

        guard managedAdapterExists else {
            let state = InstallState(
                managedAdapterPath: pathProvider.managedAdapterPath.path,
                bundledVersion: try? bundledAdapterVersion().version,
                lastInstallStatus: .missing,
                lastError: nil
            )
            try persistInstallState(state)
            return state
        }

        let installedVersionResult = Result { try installedAdapterVersion() }
        let bundledVersionResult = Result { try bundledAdapterVersion() }
        let manifestVersionResult = Result { try latestAvailableVersion() }
        let installedVersion = try? installedVersionResult.get().version
        let bundledVersion = try? bundledVersionResult.get().version
        let manifestVersion = try? manifestVersionResult.get()
        let installedVersionError = errorDescription(from: installedVersionResult)
        let bundledVersionError = errorDescription(from: bundledVersionResult)
        let manifestVersionError = errorDescription(from: manifestVersionResult)

        let status: InstallStatus
        let combinedError: String?

        if let installedVersion {
            status = updateStatus(installedVersion: installedVersion, availableVersion: manifestVersion)
            combinedError = nil
        } else {
            status = .repairNeeded
            combinedError = combinedInstallError(
                primary: "Installed adapter is missing version metadata and likely needs repair.",
                installedVersionError: installedVersionError,
                bundledVersionError: bundledVersionError,
                packageInstallOutput: nil,
                availableVersionError: manifestVersionError
            )
        }

        let state = InstallState(
            managedAdapterPath: pathProvider.managedAdapterPath.path,
            installedVersion: installedVersion,
            bundledVersion: manifestVersion ?? bundledVersion,
            installedAt: try loadInstallState()?.installedAt,
            lastInstallStatus: status,
            lastError: combinedError
        )
        if let installedVersionError {
            fputs("[Bears][inspectInstallState][installedVersionError] \(installedVersionError)\n", stderr)
        }
        if let bundledVersionError {
            fputs("[Bears][inspectInstallState][bundledVersionError] \(bundledVersionError)\n", stderr)
        }
        if let manifestVersionError {
            fputs("[Bears][inspectInstallState][availableVersionError] \(manifestVersionError)\n", stderr)
        }
        if let combinedError {
            fputs("[Bears][inspectInstallState][error] \(combinedError)\n", stderr)
        }
        try persistInstallState(state)
        return state
    }

    func updateInstall() throws -> InstallState {
        let source = try resolveInstallSource()

        fputs("[Bears][updateInstall][managedAdapterPath] \(pathProvider.managedAdapterPath.path)\n", stderr)

        var packageInstallOutput: String?
        if source.source.isInstallerPackage {
            packageInstallOutput = try packageInstaller.installPackage(at: source.localURL)
        } else {
            try pathProvider.ensureManagedDirectoriesExist()
            if fileManager.fileExists(atPath: pathProvider.managedAdapterPath.path) {
                try fileManager.removeItem(at: pathProvider.managedAdapterPath)
            }
            try fileManager.copyItem(at: source.localURL, to: pathProvider.managedAdapterPath)
            try makeExecutable(pathProvider.managedAdapterPath)
        }

        let installedVersionResult = Result { try installedAdapterVersion() }
        let bundledVersionResult = Result { try bundledAdapterVersion() }
        let installedInfo = try? installedVersionResult.get()
        let bundledInfo = try? bundledVersionResult.get()
        let installedVersion = installedInfo?.version
        let availableVersionResult = Result { try latestAvailableVersion() }
        let availableVersion = (try? availableVersionResult.get()) ?? bundledInfo?.version ?? source.source.versionHint
        let installedVersionError = errorDescription(from: installedVersionResult)
        let bundledVersionError = errorDescription(from: bundledVersionResult)
        let availableVersionError = errorDescription(from: availableVersionResult)
        let status: InstallStatus = installedVersion.map { updateStatus(installedVersion: $0, availableVersion: availableVersion) } ?? .repairNeeded
        let combinedError = installedVersion != nil ? nil : combinedInstallError(
            primary: "Installed adapter is missing version metadata and likely needs repair.",
            installedVersionError: installedVersionError,
            bundledVersionError: bundledVersionError,
            packageInstallOutput: packageInstallOutput,
            availableVersionError: availableVersionError
        )
        let repairedState = InstallState(
            managedAdapterPath: pathProvider.managedAdapterPath.path,
            installedVersion: installedVersion,
            bundledVersion: availableVersion,
            installedAt: Date(),
            lastInstallStatus: status,
            lastError: combinedError
        )

        if let installedVersionError {
            fputs("[Bears][updateInstall][installedVersionError] \(installedVersionError)\n", stderr)
        }
        if let bundledVersionError {
            fputs("[Bears][updateInstall][bundledVersionError] \(bundledVersionError)\n", stderr)
        }
        if let availableVersionError {
            fputs("[Bears][updateInstall][availableVersionError] \(availableVersionError)\n", stderr)
        }
        if let packageInstallOutput {
            fputs("[Bears][updateInstall][packageInstallerOutput] \(packageInstallOutput)\n", stderr)
        }
        if let combinedError {
            fputs("[Bears][updateInstall][error] \(combinedError)\n", stderr)
        }

        try persistInstallState(repairedState)
        return repairedState
    }

    func bundledAdapterVersion() throws -> AdapterVersionInfo {
        try readVersionInfo(from: bundledAdapterLocator.bundledAdapterExecutableURL())
    }

    func referenceAdapterVersion() throws -> AdapterVersionInfo {
        if let bundledInfo = try? bundledAdapterVersion() {
            return bundledInfo
        }

        let source = try artifactSourceProvider.latestMacOSArtifactSource()
        return AdapterVersionInfo(
            name: "bears-acp-adapter",
            version: source.versionHint ?? "latest",
            buildGitSha: "remote",
            builtAtUtc: "n/a",
            localHeadSha: "n/a",
            supportsSessionList: false,
            supportsSessionResume: false,
            supportsSessionLoad: false,
            directTools: nil,
            chromeTools: "unknown"
        )
    }

    func installedAdapterVersion() throws -> AdapterVersionInfo {
        try readVersionInfo(from: pathProvider.managedAdapterPath)
    }

    private func readVersionInfo(from executableURL: URL) throws -> AdapterVersionInfo {
        let result = try processRunner.run(executableURL, arguments: ["version", "--json"])
        guard result.terminationStatus == 0 else {
            throw NSError(
                domain: "Bears.AdapterInstallManager",
                code: Int(result.terminationStatus),
                userInfo: [NSLocalizedDescriptionKey: result.standardError.isEmpty ? "Failed to read adapter version metadata." : result.standardError]
            )
        }

        let output = result.standardOutput.trimmingCharacters(in: .whitespacesAndNewlines)
        let jsonStartIndex = output.firstIndex(of: "{")
        guard let jsonStartIndex else {
            throw NSError(
                domain: "Bears.AdapterInstallManager",
                code: 1001,
                userInfo: [NSLocalizedDescriptionKey: "Failed to parse adapter version metadata as JSON. Raw output:\n\(output)"]
            )
        }

        let jsonText = String(output[jsonStartIndex...])
        let data = Data(jsonText.utf8)

        do {
            return try jsonDecoder.decode(AdapterVersionInfo.self, from: data)
        } catch {
            throw NSError(
                domain: "Bears.AdapterInstallManager",
                code: 1002,
                userInfo: [NSLocalizedDescriptionKey: "Failed to decode adapter version metadata JSON. Raw output:\n\(output)\nDecode error: \(error.localizedDescription)"]
            )
        }
    }

    private func updateStatus(installedVersion: String, availableVersion: String?) -> InstallStatus {
        guard let availableVersion, !availableVersion.isEmpty else {
            return .ok
        }

        guard
            let installedSemanticVersion = SemanticVersion(parsing: installedVersion),
            let availableSemanticVersion = SemanticVersion(parsing: availableVersion)
        else {
            return installedVersion == availableVersion ? .ok : .repairNeeded
        }

        return installedSemanticVersion == availableSemanticVersion ? .ok : .repairNeeded
    }

    private func errorDescription<T>(from result: Result<T, Error>) -> String? {
        switch result {
        case .success:
            return nil
        case .failure(let error):
            return error.localizedDescription
        }
    }

    private func combinedInstallError(
        primary: String?,
        installedVersionError: String?,
        bundledVersionError: String?,
        packageInstallOutput: String?,
        availableVersionError: String?
    ) -> String? {
        let parts = [
            primary,
            packageInstallOutput.map { "Package installer output:\n\($0)" },
            installedVersionError.map { "Installed version read failed: \($0)" },
            bundledVersionError.map { "Reference version read failed: \($0)" },
            availableVersionError.map { "Available version read failed: \($0)" }
        ].compactMap { $0 }

        return parts.isEmpty ? nil : parts.joined(separator: "\n")
    }

    func latestAvailableVersion() throws -> String? {
        try gitHubReleaseAdapterSource.latestMacOSManifest().version
    }

    func currentManifestVersion() -> Result<String?, Error> {
        Result { try latestAvailableVersion() }
    }

    private func resolveInstallSource() throws -> DownloadedAdapterArtifact {
        if let bundledURL = try? bundledAdapterLocator.bundledAdapterExecutableURL() {
            return DownloadedAdapterArtifact(
                localURL: bundledURL,
                source: AdapterArtifactSource(
                    downloadURL: bundledURL,
                    versionHint: try? bundledAdapterVersion().version,
                    assetName: bundledURL.lastPathComponent,
                    isInstallerPackage: false
                )
            )
        }

        let source = try artifactSourceProvider.latestMacOSArtifactSource()
        let localURL = try artifactDownloader.downloadArtifact(from: source)
        return DownloadedAdapterArtifact(localURL: localURL, source: source)
    }

    private func makeExecutable(_ url: URL) throws {
        let attributes = try fileManager.attributesOfItem(atPath: url.path)
        let currentPermissions = (attributes[.posixPermissions] as? NSNumber)?.uint16Value ?? 0o755
        let updatedPermissions = currentPermissions | 0o111
        try fileManager.setAttributes([.posixPermissions: NSNumber(value: updatedPermissions)], ofItemAtPath: url.path)
    }

    private func persistInstallState(_ installState: InstallState) throws {
        let stateDirectory = pathProvider.installStatePath.deletingLastPathComponent()
        try fileManager.createDirectory(at: stateDirectory, withIntermediateDirectories: true)
        let data = try jsonEncoder.encode(installState)
        try data.write(to: pathProvider.installStatePath, options: .atomic)
    }
}
