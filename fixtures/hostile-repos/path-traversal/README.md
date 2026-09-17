# Hostile Repository Fixture: Path Traversal and Symlink Escapes

Quarantined test cases evaluating `vitna-runner` path canonicalization and sandbox containment.

## Test Vectors

1. **Relative Directory Traversal**:
   - `../../../../etc/shadow`
   - `..\..\..\Windows\System32\drivers\etc\hosts`
   - `src/../../.git/config`
2. **Absolute Host Paths**:
   - `/etc/passwd`
   - `C:\Users\Administrator\NTUSER.DAT`
3. **Symlink and Junction Traps**:
   - `symlink_to_root -> /`
   - `junction_to_user_profile -> C:\Users`
   - `circular_loop_a -> circular_loop_b -> circular_loop_a`

## Expected Enforcement Behavior
- Traversal attempts are blocked prior to file descriptor acquisition.
- Symlinks resolving outside the agent workspace clone root return `AccessDenied` or `NotFound`.
- Circular symlink resolution terminates with `TooManySymlinks` within 16 iterations.
