import Foundation

enum GitHubReleaseAdapterSourceError: LocalizedError {
    case invalidURL(String)
    case manifestNotFound(String)
    case manifestUnavailable(String)
    case invalidManifestJSON(String)
    case missingPackageURL

    var errorDescription: String? {
        switch self {
        case .invalidURL(let urlString):
            return "Invalid adapter download URL: \(urlString)"
        case .manifestNotFound(let details):
            return "macOS adapter manifest not found: \(details)"
        case .manifestUnavailable(let details):
            return "Failed to load macOS adapter manifest: \(details)"
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
        var urlResponse: URLResponse?

        var request = URLRequest(url: manifestURL)
        request.cachePolicy = .reloadIgnoringLocalCacheData

        URLSession.shared.dataTask(with: request) { data, response, error in
            responseData = data
            urlResponse = response
            responseError = error
            semaphore.signal()
        }.resume()

        semaphore.wait()

        if let responseError {
            throw GitHubReleaseAdapterSourceError.manifestNotFound(responseError.localizedDescription)
        }

        if let httpResponse = urlResponse as? HTTPURLResponse {
            switch httpResponse.statusCode {
            case 200:
                break
            case 404:
                throw GitHubReleaseAdapterSourceError.manifestNotFound("HTTP 404 at \(manifestURL.absoluteString)")
            case 500...599:
                throw GitHubReleaseAdapterSourceError.manifestUnavailable("HTTP \(httpResponse.statusCode) at \(manifestURL.absoluteString)")
            default:
                throw GitHubReleaseAdapterSourceError.manifestNotFound("HTTP \(httpResponse.statusCode) at \(manifestURL.absoluteString)")
            }
        }

        guard let responseData else {
            throw GitHubReleaseAdapterSourceError.manifestNotFound("No data returned from \(manifestURL.absoluteString)")
        }

        do {
            let decoder = JSONDecoder()
            decoder.keyDecodingStrategy = .convertFromSnakeCase
            return try decoder.decode(MacOSAdapterManifest.self, from: responseData)
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
