import Foundation

struct BearsPathResolver: AdapterPathProviding {
    private let fileManager: FileManager
    private let homeDirectory: URL

    init(
        fileManager: FileManager = .default,
        homeDirectory: URL = FileManager.default.homeDirectoryForCurrentUser
    ) {
        self.fileManager = fileManager
        self.homeDirectory = homeDirectory
    }

    var applicationSupportRoot: URL {
        homeDirectory
            .appendingPathComponent("Library", isDirectory: true)
            .appendingPathComponent("Application Support", isDirectory: true)
            .appendingPathComponent("Bears", isDirectory: true)
    }

    var managedAdapterPath: URL {
        URL(fileURLWithPath: "/Library/Application Support/Bears", isDirectory: true)
            .appendingPathComponent("adapter", isDirectory: true)
            .appendingPathComponent("bears-acp-adapter", isDirectory: false)
    }

    var installStatePath: URL {
        applicationSupportRoot
            .appendingPathComponent("state", isDirectory: true)
            .appendingPathComponent("install-state.json", isDirectory: false)
    }

    var acpLogsDirectory: URL {
        applicationSupportRoot
            .appendingPathComponent("logs", isDirectory: true)
            .appendingPathComponent("acp", isDirectory: true)
    }

    var adapterDirectory: URL {
        managedAdapterPath.deletingLastPathComponent()
    }

    var stateDirectory: URL {
        installStatePath.deletingLastPathComponent()
    }

    func ensureManagedDirectoriesExist() throws {
        try fileManager.createDirectory(at: adapterDirectory, withIntermediateDirectories: true)
        try fileManager.createDirectory(at: stateDirectory, withIntermediateDirectories: true)
    }
}
