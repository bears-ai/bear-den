import Foundation

enum GitHubReleaseAdapterSourceError: LocalizedError {
    case invalidURL(String)
    case manifestDownloadFailed(String)
    case invalidManifestJSON(String)
    case missingPackageURL

    var errorDescription: String? {
        switch self {
        case .invalidURL(let urlString):
            return "Invalid adapter download URL: \(urlString)"
        case .manifestDownloadFailed(let details):
            return "Failed to download macOS adapter manifest: \(details)"
        case .invalidManifestJSON(let details):
            return "Failed to decode macOS adapter manifest JSON: \(details)"
        case .missingPackageURL:
            return "The macOS adapter manifest does not contain a pkg_url value."
        }
    }
}

struct MacOSAdapterManifest: Decodable {
    let version: String?
    let pkgURL: String
    let pkgSha256: String?
    let releaseNotesURL: String?
}

struct GitHubReleaseAdapterSource: AdapterArtifactSourceProviding {
    private let environment: ProcessInfo

    init(environment: ProcessInfo = .processInfo) {
        self.environment = environment
    }

    func latestMacOSArtifactSource() throws -> AdapterArtifactSource {
        if let configuredDownloadURLString = environment.environment["BEARS_ADAPTER_DOWNLOAD_URL"] {
            return try adapterSource(from: configuredDownloadURLString, versionHint: nil)
        }

        let manifest = try latestMacOSManifest()
        guard !manifest.pkgURL.isEmpty else {
            throw GitHubReleaseAdapterSourceError.missingPackageURL
        }

        let manifestURL = try manifestURL()
        let packageURLString = resolvedPackageURLString(from: manifest.pkgURL, manifestURL: manifestURL)
        return try adapterSource(from: packageURLString, versionHint: manifest.version)
    }

    func latestMacOSManifest() throws -> MacOSAdapterManifest {
        let manifestURL = try manifestURL()
        return try fetchManifest(from: manifestURL)
    }

    private func manifestURL() throws -> URL {
        let manifestURLString = environment.environment["BEARS_ADAPTER_MANIFEST_URL"]
            ?? "https://bears-ai.github.io/bear-den/bears-acp-adapter/stable/macos.json"

        guard let manifestURL = URL(string: manifestURLString) else {
            throw GitHubReleaseAdapterSourceError.invalidURL(manifestURLString)
        }

        return manifestURL
    }

    private func adapterSource(from urlString: String, versionHint: String?) throws -> AdapterArtifactSource {
        guard let downloadURL = URL(string: urlString) else {
            throw GitHubReleaseAdapterSourceError.invalidURL(urlString)
        }

        return AdapterArtifactSource(
            downloadURL: downloadURL,
            versionHint: versionHint,
            assetName: downloadURL.lastPathComponent.isEmpty ? "bears-acp-adapter-aarch64-apple-darwin.pkg" : downloadURL.lastPathComponent,
            isInstallerPackage: downloadURL.pathExtension == "pkg"
        )
    }

    private func fetchManifest(from manifestURL: URL) throws -> MacOSAdapterManifest {
        let semaphore = DispatchSemaphore(value: 0)
        var responseData: Data?
        var responseError: Error?

        URLSession.shared.dataTask(with: manifestURL) { data, _, error in
            responseData = data
            responseError = error
            semaphore.signal()
        }.resume()

        semaphore.wait()

        if let responseError {
            throw GitHubReleaseAdapterSourceError.manifestDownloadFailed(responseError.localizedDescription)
        }

        guard let responseData else {
            throw GitHubReleaseAdapterSourceError.manifestDownloadFailed("No data returned from \(manifestURL.absoluteString)")
        }

        do {
            return try JSONDecoder().decode(MacOSAdapterManifest.self, from: responseData)
        } catch {
            let raw = String(data: responseData, encoding: .utf8) ?? "<non-utf8 response>"
            throw GitHubReleaseAdapterSourceError.invalidManifestJSON("\(error.localizedDescription). Raw response: \(raw)")
        }
    }

    private func resolvedPackageURLString(from pkgURL: String, manifestURL: URL) -> String {
        if URL(string: pkgURL)?.scheme != nil {
            return pkgURL
        }

        return URL(string: pkgURL, relativeTo: manifestURL)?.absoluteURL.absoluteString ?? pkgURL
    }
}
