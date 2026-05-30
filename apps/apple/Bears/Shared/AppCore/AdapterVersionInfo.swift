import Foundation

struct AdapterVersionInfo: Codable, Equatable {
    let name: String
    let version: String
    let buildGitSha: String
    let builtAtUtc: String
    let localHeadSha: String
    let supportsSessionList: Bool
    let supportsSessionResume: Bool
    let supportsSessionLoad: Bool
    let directTools: [String: Bool]?
    let chromeTools: String
}
