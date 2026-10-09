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
                primary: "Unable to inspect installed armature version: \(installedVersionError?.components(separatedBy: .newlines).first ?? "no readable metadata returned")",
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
        try persistInstallState(state)
        return state
    }

    func updateInstall() throws -> AdapterInstallUpdateResult {
        let source = try resolveInstallSource()

        if source.source.isInstallerPackage {
            let packageInstallOutput = try packageInstaller.installPackage(at: source.localURL)
            return .installerOpened(message: packageInstallOutput)
        }

        try pathProvider.ensureManagedDirectoriesExist()
        if fileManager.fileExists(atPath: pathProvider.managedAdapterPath.path) {
            try fileManager.removeItem(at: pathProvider.managedAdapterPath)
        }
        try fileManager.copyItem(at: source.localURL, to: pathProvider.managedAdapterPath)
        try makeExecutable(pathProvider.managedAdapterPath)

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
            primary: "Unable to inspect installed armature version: \(installedVersionError?.components(separatedBy: .newlines).first ?? "no readable metadata returned")",
            installedVersionError: installedVersionError,
            bundledVersionError: bundledVersionError,
            packageInstallOutput: nil,
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

        try persistInstallState(repairedState)
        return .installed(repairedState)
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
            name: "bear-armature",
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
        try AdapterVersionReader(processRunner: processRunner).read(from: executableURL)
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

    func installedAdapterSnapshot() -> InstalledAdapterSnapshot {
        let path = pathProvider.managedAdapterPath.path
        let exists = fileManager.fileExists(atPath: path)
        guard exists else {
            return InstalledAdapterSnapshot(version: nil, exists: false, fileSize: nil, modificationDate: nil)
        }

        let attributes = try? fileManager.attributesOfItem(atPath: path)
        let fileSize = (attributes?[.size] as? NSNumber)?.uint64Value
        let modificationDate = attributes?[.modificationDate] as? Date
        let version = (try? installedAdapterVersion().version)

        return InstalledAdapterSnapshot(
            version: version,
            exists: true,
            fileSize: fileSize,
            modificationDate: modificationDate
        )
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
