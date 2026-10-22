//! Regression suite for the owner-private directory primitive. The
//! privilege-requiring Windows negative cases (icacls / `mklink /J`) skip
//! with a clear message when the privilege is missing.

mod create_tests;
mod verify_tests;
