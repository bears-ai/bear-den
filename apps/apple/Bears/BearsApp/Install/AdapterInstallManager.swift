import Foundation

struct AdapterInstallManager: AdapterInstallManaging, AdapterVersionProviding {
    private let pathProvider: BearsPathResolver
    private let bundledAdapterLocator: BundledAdapterLocating
    private let processRunner: ProcessRunning
    private let fileManager: FileManager
    private let jsonDecoder: JSONDecoder
    private let jsonEncoder: JSONEncoder

    init(
        pathProvider: BearsPathResolver = BearsPathResolver(),
        bundledAdapterLocator: BundledAdapterLocating = BundledAdapterLocator(),
        processRunner: ProcessRunning = FoundationProcessRunner(),
        fileManager: FileManager = .default
    ) {
        self.pathProvider = pathProvider
        self.bundledAdapterLocator = bundledAdapterLocator
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

        let bundledAdapterURL = try bundledAdapterLocator.bundledAdapterExecutableURL()
        if fileManager.fileExists(atPath: pathProvider.managedAdapterPath.path) {
            try fileManager.removeItem(at: pathProvider.managedAdapterPath)
        }
        try fileManager.copyItem(at: bundledAdapterURL, to: pathProvider.managedAdapterPath)
        try makeExecutable(pathProvider.managedAdapterPath)

        let installedVersion = try? installedAdapterVersion().version
        let bundledVersion = try? bundledAdapterVersion().version
        let isCompatible = versionsAreCompatible(installedVersion: installedVersion, bundledVersion: bundledVersion)
        let status: InstallStatus = isCompatible ? .ok : .repairNeeded
        let repairedState = InstallState(
            managedAdapterPath: pathProvider.managedAdapterPath.path,
            installedVersion: installedVersion,
            bundledVersion: bundledVersion,
            installedAt: Date(),
            lastInstallStatus: status,
            lastError: status == .ok ? nil : "Installed adapter is incompatible with bundled adapter. Patch-level differences are allowed, but major/minor versions must match."
        )

        try persistInstallState(repairedState)
        return repairedState
    }

    func bundledAdapterVersion() throws -> AdapterVersionInfo {
        try readVersionInfo(from: bundledAdapterLocator.bundledAdapterExecutableURL())
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

        guard
            let installedSemanticVersion = SemanticVersion(parsing: installedVersion),
            let bundledSemanticVersion = SemanticVersion(parsing: bundledVersion)
        else {
            return installedVersion == bundledVersion
        }

        return installedSemanticVersion.isCompatiblePatchwise(with: bundledSemanticVersion)
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
