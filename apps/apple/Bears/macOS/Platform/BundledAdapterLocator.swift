import Foundation

protocol BundledAdapterLocating {
    func bundledAdapterExecutableURL() throws -> URL
}

enum BundledAdapterLocatorError: LocalizedError {
    case missingResource

    var errorDescription: String? {
        switch self {
        case .missingResource:
            return "The Bears app bundle does not contain a bundled bears-acp-adapter executable yet."
        }
    }
}

struct BundledAdapterLocator: BundledAdapterLocating {
    let bundle: Bundle

    init(bundle: Bundle = .main) {
        self.bundle = bundle
    }

    func bundledAdapterExecutableURL() throws -> URL {
        if let url = bundle.url(forResource: "bears-acp-adapter", withExtension: nil) {
            return url
        }

        if let resourcesURL = bundle.resourceURL {
            let candidate = resourcesURL
                .appendingPathComponent("Adapter", isDirectory: true)
                .appendingPathComponent("bears-acp-adapter", isDirectory: false)
            if FileManager.default.fileExists(atPath: candidate.path) {
                return candidate
            }
        }

        throw BundledAdapterLocatorError.missingResource
    }
}
