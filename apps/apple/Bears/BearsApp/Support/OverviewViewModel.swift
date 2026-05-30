import Foundation

@MainActor
final class OverviewViewModel: ObservableObject {
    @Published private(set) var installState: InstallState?
    @Published private(set) var managedAdapterPath: String
    @Published private(set) var bundledVersion: String = "Unavailable"
    @Published private(set) var installedVersion: String = "Unavailable"
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
            let state = try installManager.inspectInstallState()
            installState = state
            bundledVersion = state.bundledVersion ?? "Unavailable"
            installedVersion = state.installedVersion ?? "Unavailable"
            statusText = Self.statusText(for: state.lastInstallStatus)
            lastError = state.lastError
        } catch {
            statusText = "Error"
            lastError = error.localizedDescription
        }
    }

    func repairInstall() {
        do {
            let state = try installManager.repairInstall()
            installState = state
            bundledVersion = state.bundledVersion ?? "Unavailable"
            installedVersion = state.installedVersion ?? "Unavailable"
            statusText = Self.statusText(for: state.lastInstallStatus)
            lastError = state.lastError
        } catch {
            statusText = "Error"
            lastError = error.localizedDescription
        }
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
