//! Gallery's own chrome, as scenes (`just shell-scenes`).
//!
//! Most of its states only appear when a run reaches them — a build has to fail before anyone sees
//! a failure — so these pose every one instead. Inside the crate because the chrome is
//! `pub(crate)`, so what is on screen is the component and not a copy that drifts; behind a
//! feature because a consumer's sidebar is theirs.

mod chrome;
mod hot;
mod icons;
mod panels;
mod sidebar;
