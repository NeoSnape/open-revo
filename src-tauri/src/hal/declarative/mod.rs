// Declarative (JSON manifest) drivers targeted other MECHREVO platforms (Xingyao, Wujie, BLD).
// This fork only runs on the Tongfang ACPI 极光 X Pro, so no manifest ever matches. The manifest
// types are kept so remaining capability checks still type-check; external driver JSONs under
// %APPDATA%\OpenRevo\plugins\drivers are deliberately no longer loaded.

pub mod model_manifest;

pub use model_manifest::ModelManifest;

pub fn load_matched_manifest() -> Option<ModelManifest> {
    None
}
