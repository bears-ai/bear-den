import Foundation
#if os(macOS)
import AppKit
#endif

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
    @Published private(set) var bundledVersionCopied = false
    @Published private(set) var installedVersionCopied = false

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
            let referenceInfoResult = Result { try installManager.referenceAdapterVersion() }
            let installedInfoResult = Result { try installManager.installedAdapterVersion() }
            let referenceInfo = try? referenceInfoResult.get()
            let installedInfo = try? installedInfoResult.get()
            let state = try installManager.inspectInstallState()
            installState = state
            bundledVersion = state.bundledVersion ?? referenceInfo?.version ?? "Unavailable"
            installedVersion = state.installedVersion ?? installedInfo?.version ?? "Unavailable"
            bundledVersionDetails = Self.versionDetails(from: referenceInfo)
            installedVersionDetails = Self.versionDetails(from: installedInfo)
            statusText = Self.statusText(for: state.lastInstallStatus)
            let combinedError = Self.combinedError(
                primary: state.lastError,
                referenceVersionError: Self.errorDescription(from: referenceInfoResult, prefix: "Reference version read failed"),
                installedVersionError: Self.errorDescription(from: installedInfoResult, prefix: "Installed version read failed")
            )
            lastError = installedInfo != nil ? nil : Self.shortVisibleError(from: combinedError)
            if let combinedError {
                fputs("[Bears][OverviewViewModel][refresh][visibleError] \(combinedError)\n", stderr)
            }
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
            let referenceInfoResult = Result { try installManager.referenceAdapterVersion() }
            let installedInfoResult = Result { try installManager.installedAdapterVersion() }
            let referenceInfo = try? referenceInfoResult.get()
            let installedInfo = try? installedInfoResult.get()
            installState = state
            bundledVersion = state.bundledVersion ?? referenceInfo?.version ?? "Unavailable"
            installedVersion = state.installedVersion ?? installedInfo?.version ?? "Unavailable"
            bundledVersionDetails = Self.versionDetails(from: referenceInfo)
            installedVersionDetails = Self.versionDetails(from: installedInfo)
            statusText = Self.statusText(for: state.lastInstallStatus)
            let combinedError = Self.combinedError(
                primary: state.lastError,
                referenceVersionError: Self.errorDescription(from: referenceInfoResult, prefix: "Reference version read failed"),
                installedVersionError: Self.errorDescription(from: installedInfoResult, prefix: "Installed version read failed")
            )
            lastError = installedInfo != nil ? nil : Self.shortVisibleError(from: combinedError)
            if let combinedError {
                fputs("[Bears][OverviewViewModel][repairInstall][visibleError] \(combinedError)\n", stderr)
            }
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
            "chromeTools=\(info.chromeTools)",
            "directTools=\(info.directTools?.count ?? 0) entries"
        ].joined(separator: "\n")
    }

    private static func errorDescription<T>(from result: Result<T, Error>, prefix: String) -> String? {
        switch result {
        case .success:
            return nil
        case .failure(let error):
            return "\(prefix): \(error.localizedDescription)"
        }
    }

    func versionDetails(forInstalledVersion: Bool) -> String {
        forInstalledVersion ? installedVersionDetails : bundledVersionDetails
    }

    func copyVersionDetails(forInstalledVersion: Bool) {
        #if os(macOS)
        let details = versionDetails(forInstalledVersion: forInstalledVersion)
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(details, forType: .string)
        #endif

        if forInstalledVersion {
            installedVersionCopied = true
        } else {
            bundledVersionCopied = true
        }

        Task {
            try? await Task.sleep(nanoseconds: 1_000_000_000)
            if forInstalledVersion {
                installedVersionCopied = false
            } else {
                bundledVersionCopied = false
            }
        }
    }

    private static func combinedError(primary: String?, referenceVersionError: String?, installedVersionError: String?) -> String? {
        let parts = [primary, referenceVersionError, installedVersionError].compactMap { $0 }
        return parts.isEmpty ? nil : parts.joined(separator: "\n")
    }

    private static func shortVisibleError(from error: String?) -> String? {
        guard let error, !error.isEmpty else {
            return nil
        }

        let firstLine = error
            .split(separator: "\n", omittingEmptySubsequences: false)
            .first
            .map(String.init)?
            .trimmingCharacters(in: .whitespacesAndNewlines)

        guard let firstLine, !firstLine.isEmpty else {
            return "Error details available"
        }

        return firstLine.count > 160 ? String(firstLine.prefix(157)) + "..." : firstLine
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
