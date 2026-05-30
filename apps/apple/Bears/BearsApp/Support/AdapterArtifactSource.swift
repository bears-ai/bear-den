import Foundation

struct AdapterArtifactSource: Equatable {
    let downloadURL: URL
    let versionHint: String?
    let assetName: String
}

protocol AdapterArtifactSourceProviding {
    func latestMacOSArtifactSource() throws -> AdapterArtifactSource
}
