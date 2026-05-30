import Foundation

@MainActor
final class OverviewViewModel: ObservableObject {
    @Published private(set) var installState: InstallState?
    @Published private(set) var managedAdapterPath: String
    @Published private(set) var bundledVersion: String = "Unavailable"
    @Published private(set) var installedVersion: String = "Unavailable"
    @Published private(set) var bundledVersionDetails: String = "Unavailable"
    @Published private(set) var installedVersionDetails: String = "Unavailable"
    @Published private(set) var statusText: String = "Not checked"
    @Published private(set) var lastError: String?

    private let installManager: AdapterInstallManager
    private let pathProvider: BearsPathResolver

    init(
        installManager: AdapterInstallManager = AdapterInstallManager(),
        pathProvider: BearsPathResolver = BearsPathResolver()
    ) {
        self.installManager = installManager
        self.pathProvider = pathProvider
        self.managedAdapterPath = pathProvider.managedAdapterPath.path
    }

    func refresh() {
        do {
            let referenceInfo = try? installManager.referenceAdapterVersion()
            let installedInfo = try? installManager.installedAdapterVersion()
            let state = try installManager.inspectInstallState()
            installState = state
            bundledVersion = state.bundledVersion ?? referenceInfo?.version ?? "Unavailable"
            installedVersion = state.installedVersion ?? installedInfo?.version ?? "Unavailable"
            bundledVersionDetails = Self.versionDetails(from: referenceInfo)
            installedVersionDetails = Self.versionDetails(from: installedInfo)
            statusText = Self.statusText(for: state.lastInstallStatus)
            lastError = state.lastError
        } catch {
            statusText = "Error"
            lastError = error.localizedDescription
            bundledVersionDetails = "Unavailable"
            installedVersionDetails = "Unavailable"
            fputs("[Bears][refresh] \(error.localizedDescription)\n", stderr)
        }
    }

    func repairInstall() {
        do {
            let state = try installManager.repairInstall()
            let referenceInfo = try? installManager.referenceAdapterVersion()
            let installedInfo = try? installManager.installedAdapterVersion()
            installState = state
            bundledVersion = state.bundledVersion ?? referenceInfo?.version ?? "Unavailable"
            installedVersion = state.installedVersion ?? installedInfo?.version ?? "Unavailable"
            bundledVersionDetails = Self.versionDetails(from: referenceInfo)
            installedVersionDetails = Self.versionDetails(from: installedInfo)
            statusText = Self.statusText(for: state.lastInstallStatus)
            lastError = state.lastError
        } catch {
            statusText = "Error"
            lastError = error.localizedDescription
            bundledVersionDetails = "Unavailable"
            installedVersionDetails = "Unavailable"
            fputs("[Bears][repairInstall] \(error.localizedDescription)\n", stderr)
        }
    }

    private static func versionDetails(from info: AdapterVersionInfo?) -> String {
        guard let info else {
            return "Unavailable"
        }

        return [
            "version=\(info.version)",
            "buildGitSha=\(info.buildGitSha)",
            "localHeadSha=\(info.localHeadSha)",
            "builtAtUTC=\(info.builtAtUtc)",
            "chromeTools=\(info.chromeTools)"
        ].joined(separator: "\n")
    }

    private static func statusText(for status: InstallStatus) -> String {
        switch status {
        case .ok:
            return "Installed"
        case .missing:
            return "Missing"
        case .repairNeeded:
            return "Repair Needed"
        case .error:
            return "Error"
        }
    }
}
