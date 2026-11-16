//! ONLYOFFICE integration (docs/specs/2026-11-16-onlyoffice-integration-design.md): office files
//! of tasks and Docs pages open in an ONLYOFFICE Docs editor, which saves them back to Orbit.
//! A feature of the `evolv3ai/orbit` fork: everything of it lives in this module.

pub mod editor;
pub mod files;
pub mod jwt;
pub mod routes;
pub mod settings;
pub mod token;
pub mod versions;
