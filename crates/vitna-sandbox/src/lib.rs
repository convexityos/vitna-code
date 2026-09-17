//! Platform-specific OS sandbox enforcement abstractions (Bubblewrap, AppContainer, Landlock).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxGuarantee {
    ReadOnly,
    Guarded,
    Strong,
    FullAccess,
}