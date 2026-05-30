import Foundation

struct AdapterInstallManager: AdapterInstallManaging, AdapterVersionProviding {
    private let pathProvider: BearsPathResolver
    private let bundledAdapterLocator: BundledAdapterLocating
    private let artifactSourceProvider: AdapterArtifactSourceProviding
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
        artifactDownloader: AdapterArtifactDownloading = URLSessionAdapterArtifactDownloader(),
        packageInstaller: AdapterPackageInstalling = InstallerAppAdapterPackageInstaller(),
        processRunner: ProcessRunning = FoundationProcessRunner(),
        fileManager: FileManager = .default
    ) {
        self.pathProvider = pathProvider
        self.bundledAdapterLocator = bundledAdapterLocator
        self.artifactSourceProvider = artifactSourceProvider
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
        let installedVersion = try? installedVersionResult.get().version
        let bundledVersion = try? bundledVersionResult.get().version
        let installedVersionError = errorDescription(from: installedVersionResult)
        let bundledVersionError = errorDescription(from: bundledVersionResult)
        let isCompatible = versionsAreCompatible(installedVersion: installedVersion, bundledVersion: bundledVersion)
        let status: InstallStatus = isCompatible ? .ok : .repairNeeded
        let mismatchDetails = compatibilityDetails(installedVersion: installedVersion, referenceVersion: bundledVersion)
        let combinedError = combinedInstallError(
            primary: status == .ok ? nil : mismatchDetails,
            installedVersionError: installedVersionError,
            bundledVersionError: bundledVersionError,
            packageInstallOutput: nil
        )
        let state = InstallState(
            managedAdapterPath: pathProvider.managedAdapterPath.path,
            installedVersion: installedVersion,
            bundledVersion: bundledVersion,
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
        if status != .ok {
            fputs("[Bears][inspectInstallState][compatibility] \(mismatchDetails)\n", stderr)
        }
        if let combinedError {
            fputs("[Bears][inspectInstallState][error] \(combinedError)\n", stderr)
        }
        try persistInstallState(state)
        return state
    }

    func repairInstall() throws -> InstallState {
        let source = try resolveInstallSource()

        fputs("[Bears][repairInstall][managedAdapterPath] \(pathProvider.managedAdapterPath.path)\n", stderr)

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
        let referenceVersion = bundledInfo?.version ?? source.source.versionHint ?? installedVersion
        let installedVersionError = errorDescription(from: installedVersionResult)
        let bundledVersionError = errorDescription(from: bundledVersionResult)
        let isCompatible = versionsAreCompatible(installedVersion: installedVersion, bundledVersion: referenceVersion)
        let status: InstallStatus = isCompatible ? .ok : .repairNeeded
        let mismatchDetails = compatibilityDetails(installedVersion: installedVersion, referenceVersion: referenceVersion)
        let combinedError = combinedInstallError(
            primary: status == .ok ? nil : mismatchDetails,
            installedVersionError: installedVersionError,
            bundledVersionError: bundledVersionError,
            packageInstallOutput: packageInstallOutput
        )
        let repairedState = InstallState(
            managedAdapterPath: pathProvider.managedAdapterPath.path,
            installedVersion: installedVersion,
            bundledVersion: referenceVersion,
            installedAt: Date(),
            lastInstallStatus: status,
            lastError: combinedError
        )

        if let installedVersionError {
            fputs("[Bears][repairInstall][installedVersionError] \(installedVersionError)\n", stderr)
        }
        if let bundledVersionError {
            fputs("[Bears][repairInstall][bundledVersionError] \(bundledVersionError)\n", stderr)
        }
        if status != .ok {
            fputs("[Bears][repairInstall][compatibility] \(mismatchDetails)\n", stderr)
        }
        if let packageInstallOutput {
            fputs("[Bears][repairInstall][packageInstallerOutput] \(packageInstallOutput)\n", stderr)
        }
        if let combinedError {
            fputs("[Bears][repairInstall][error] \(combinedError)\n", stderr)
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

    private func compatibilityDetails(installedVersion: String?, referenceVersion: String?) -> String {
        let installed = installedVersion ?? "nil"
        let reference = referenceVersion ?? "nil"

        if reference == "latest" {
            return "Installed adapter version compatibility check bypassed because the app reference version is 'latest'. installed=\(installed), reference=\(reference)"
        }

        guard let installedVersion, let referenceVersion else {
            return "Installed adapter is incompatible with the app's reference adapter version because one or both versions are unavailable. installed=\(installed), reference=\(reference)"
        }

        guard
            let installedSemanticVersion = SemanticVersion(parsing: installedVersion),
            let referenceSemanticVersion = SemanticVersion(parsing: referenceVersion)
        else {
            return "Installed adapter is incompatible with the app's reference adapter version. Non-semver fallback comparison failed. installed=\(installedVersion), reference=\(referenceVersion)"
        }

        return "Installed adapter is incompatible with the app's reference adapter version. Patch-level differences are allowed, but major/minor versions must match. installed=\(installedVersion) [major=\(installedSemanticVersion.major), minor=\(installedSemanticVersion.minor), patch=\(installedSemanticVersion.patch)] reference=\(referenceVersion) [major=\(referenceSemanticVersion.major), minor=\(referenceSemanticVersion.minor), patch=\(referenceSemanticVersion.patch)]"
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
        packageInstallOutput: String?
    ) -> String? {
        let parts = [
            primary,
            packageInstallOutput.map { "Package installer output:\n\($0)" },
            installedVersionError.map { "Installed version read failed: \($0)" },
            bundledVersionError.map { "Reference version read failed: \($0)" }
        ].compactMap { $0 }

        return parts.isEmpty ? nil : parts.joined(separator: "\n")
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
