import BearsClient

/// Memory-only recovery after expiry, never cross-account persistence. Explicit sign-out erases this.
struct ReauthorizationDrafts: Sendable {
    let configuration: ClientConfiguration
    let subject: SubjectID
    let composers: [ChatURI: ComposerState]
}
