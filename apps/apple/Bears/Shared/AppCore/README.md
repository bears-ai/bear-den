# AppCore

Shared app/domain layer for the Bears Apple app.

This layer should stay portable across macOS and any future iOS app where possible.

Planned contents:

- app/domain models;
- adapter install/update state models;
- version and status models;
- log query/filter models;
- view models and use-case orchestration;
- protocol seams for platform-specific services.

Initial protocol seams to introduce in the first implementation slice:

- `AdapterInstallationManaging`
- `AdapterVersionProviding`
- `AdapterPathProviding`
- `DiagnosticsProviding`
