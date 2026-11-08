//! Sandbox policies, probes and workloads.
//!
//! The files are modules of one test target so they link once; nextest still
//! runs every test in its own process.

#[path = "common/mod.rs"]
mod common;

mod sandbox_off_test;
mod sandbox_page_helper_test;
mod sandbox_page_test;
mod sandbox_policy_test;
mod sandbox_probe_test;
mod sandbox_protocol_test;
mod sandbox_status_test;
mod sandbox_supervisor_test;
mod sandbox_unavailable_test;
mod sandbox_wedge_test;
mod sandbox_workload_test;
