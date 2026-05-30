import Foundation

protocol AdapterInstallManaging {
    func loadInstallState() throws -> InstallState?
    func inspectInstallState() throws -> InstallState
    func repairInstall() throws -> InstallState
}

protocol AdapterVersionProviding {
    func bundledAdapterVersion() throws -> AdapterVersionInfo
    func installedAdapterVersion() throws -> AdapterVersionInfo
}

protocol AdapterPathProviding {
    var applicationSupportRoot: URL { get }
    var managedAdapterPath: URL { get }
    var installStatePath: URL { get }
    var acpLogsDirectory: URL { get }
}
