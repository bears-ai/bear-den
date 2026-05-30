import Foundation

struct AdapterInstallManager: AdapterInstallManaging {
    private let pathProvider: BearsPathResolver
    private let fileManager: FileManager
    private let jsonDecoder: JSONDecoder
    private let jsonEncoder: JSONEncoder

    init(
        pathProvider: BearsPathResolver = BearsPathResolver(),
        fileManager: FileManager = .default
    ) {
        self.pathProvider = pathProvider
        self.fileManager = fileManager

        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        self.jsonDecoder = decoder

        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        encoder.dateEncodingStrategy = .iso8601
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
        if let state = try loadInstallState() {
            return state
        }

        return InstallState(
            managedAdapterPath: pathProvider.managedAdapterPath.path,
            lastInstallStatus: fileManager.fileExists(atPath: pathProvider.managedAdapterPath.path) ? .repairNeeded : .missing,
            lastError: nil
        )
    }

    func repairInstall() throws -> InstallState {
        try pathProvider.ensureManagedDirectoriesExist()

        let repairedState = InstallState(
            managedAdapterPath: pathProvider.managedAdapterPath.path,
            installedVersion: nil,
            bundledVersion: nil,
            installedAt: Date(),
            lastInstallStatus: .repairNeeded,
            lastError: "Adapter copy/install wiring not implemented yet."
        )

        try persistInstallState(repairedState)
        return repairedState
    }

    private func persistInstallState(_ installState: InstallState) throws {
        let data = try jsonEncoder.encode(installState)
        try data.write(to: pathProvider.installStatePath, options: .atomic)
    }
}
