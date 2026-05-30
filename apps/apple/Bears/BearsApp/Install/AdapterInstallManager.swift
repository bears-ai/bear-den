import Foundation

struct AdapterInstallManager: AdapterInstallManaging, AdapterVersionProviding {
    private let pathProvider: BearsPathResolver
    private let bundledAdapterLocator: BundledAdapterLocating
    private let artifactSourceProvider: AdapterArtifactSourceProviding
    private let artifactDownloader: AdapterArtifactDownloading
    private let processRunner: ProcessRunning
    private let fileManager: FileManager
    private let jsonDecoder: JSONDecoder
    private let jsonEncoder: JSONEncoder

    init(
        pathProvider: BearsPathResolver = BearsPathResolver(),
        bundledAdapterLocator: BundledAdapterLocating = BundledAdapterLocator(),
        artifactSourceProvider: AdapterArtifactSourceProviding = GitHubReleaseAdapterSource(),
        artifactDownloader: AdapterArtifactDownloading = URLSessionAdapterArtifactDownloader(),
        processRunner: ProcessRunning = FoundationProcessRunner(),
        fileManager: FileManager = .default
    ) {
        self.pathProvider = pathProvider
        self.bundledAdapterLocator = bundledAdapterLocator
        self.artifactSourceProvider = artifactSourceProvider
        self.artifactDownloader = artifactDownloader
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

        let installedVersion = try? installedAdapterVersion().version
        let bundledVersion = try? bundledAdapterVersion().version
        let isCompatible = versionsAreCompatible(installedVersion: installedVersion, bundledVersion: bundledVersion)
        let status: InstallStatus = isCompatible ? .ok : .repairNeeded
        let state = InstallState(
            managedAdapterPath: pathProvider.managedAdapterPath.path,
            installedVersion: installedVersion,
            bundledVersion: bundledVersion,
            installedAt: try loadInstallState()?.installedAt,
            lastInstallStatus: status,
            lastError: status == .ok ? nil : "Managed adapter is incompatible with bundled adapter. Patch-level differences are allowed, but major/minor versions must match."
        )
        try persistInstallState(state)
        return state
    }

    func repairInstall() throws -> InstallState {
        try pathProvider.ensureManagedDirectoriesExist()

        let sourceAdapterURL = try resolveInstallSourceAdapterURL()
        if fileManager.fileExists(atPath: pathProvider.managedAdapterPath.path) {
            try fileManager.removeItem(at: pathProvider.managedAdapterPath)
        }
        try fileManager.copyItem(at: sourceAdapterURL, to: pathProvider.managedAdapterPath)
        try makeExecutable(pathProvider.managedAdapterPath)

        let installedInfo = try? installedAdapterVersion()
        let bundledInfo = try? bundledAdapterVersion()
        let installedVersion = installedInfo?.version
        let referenceVersion = bundledInfo?.version ?? installedVersion
        let isCompatible = versionsAreCompatible(installedVersion: installedVersion, bundledVersion: referenceVersion)
        let status: InstallStatus = isCompatible ? .ok : .repairNeeded
        let repairedState = InstallState(
            managedAdapterPath: pathProvider.managedAdapterPath.path,
            installedVersion: installedVersion,
            bundledVersion: referenceVersion,
            installedAt: Date(),
            lastInstallStatus: status,
            lastError: status == .ok ? nil : "Installed adapter is incompatible with the app's reference adapter version. Patch-level differences are allowed, but major/minor versions must match."
        )

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

        let data = Data(result.standardOutput.utf8)
        return try jsonDecoder.decode(AdapterVersionInfo.self, from: data)
    }

    private func versionsAreCompatible(installedVersion: String?, bundledVersion: String?) -> Bool {
        guard let installedVersion, let bundledVersion else {
            return false
        }

        if bundledVersion == "latest" {
            return true
        }

        guard
            let installedSemanticVersion = SemanticVersion(parsing: installedVersion),
            let bundledSemanticVersion = SemanticVersion(parsing: bundledVersion)
        else {
            return installedVersion == bundledVersion
        }

        return installedSemanticVersion.isCompatiblePatchwise(with: bundledSemanticVersion)
    }

    private func resolveInstallSourceAdapterURL() throws -> URL {
        if let bundledURL = try? bundledAdapterLocator.bundledAdapterExecutableURL() {
            return bundledURL
        }

        let source = try artifactSourceProvider.latestMacOSArtifactSource()
        return try artifactDownloader.downloadArtifact(from: source)
    }

    private func makeExecutable(_ url: URL) throws {
        let attributes = try fileManager.attributesOfItem(atPath: url.path)
        let currentPermissions = (attributes[.posixPermissions] as? NSNumber)?.uint16Value ?? 0o755
        let updatedPermissions = currentPermissions | 0o111
        try fileManager.setAttributes([.posixPermissions: NSNumber(value: updatedPermissions)], ofItemAtPath: url.path)
    }

    private func persistInstallState(_ installState: InstallState) throws {
        let data = try jsonEncoder.encode(installState)
        try data.write(to: pathProvider.installStatePath, options: .atomic)
    }
}
