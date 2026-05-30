import Foundation

struct InstallState: Codable, Equatable {
    static let currentSchemaVersion = 1

    let schemaVersion: Int
    let managedAdapterPath: String
    let installedVersion: String?
    let bundledVersion: String?
    let installedAt: Date?
    let lastInstallStatus: InstallStatus
    let lastError: String?

    init(
        schemaVersion: Int = InstallState.currentSchemaVersion,
        managedAdapterPath: String,
        installedVersion: String? = nil,
        bundledVersion: String? = nil,
        installedAt: Date? = nil,
        lastInstallStatus: InstallStatus,
        lastError: String? = nil
    ) {
        self.schemaVersion = schemaVersion
        self.managedAdapterPath = managedAdapterPath
        self.installedVersion = installedVersion
        self.bundledVersion = bundledVersion
        self.installedAt = installedAt
        self.lastInstallStatus = lastInstallStatus
        self.lastError = lastError
    }
}

enum InstallStatus: String, Codable, Equatable {
    case ok
    case missing
    case repairNeeded = "repair_needed"
    case error
}
