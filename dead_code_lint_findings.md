# Dead Code Lint Implementation and Tests

## Overview

The `dead_code` lint detects unused, unexported items in Rust code and suggests using `#[allow(dead_code)]` to suppress warnings.

## Implementation

### Lint Declaration

**File**: `compiler/rustc_lint_defs/src/builtin.rs:716-756`

```rust
declare_lint! {
    /// The `dead_code` lint detects unused, unexported items.
    pub DEAD_CODE,
    Warn,
    "detect unused, unexported items"
}
```

- **Default Level**: `Warn`
- **Registered in**: `HardwiredLints` lint pass (line 34)

### Main Implementation

**File**: `compiler/rustc_passes/src/dead.rs`

This file implements the complete dead code analysis using reachability analysis.

#### Key Components

1. **Live Symbol Marking** (lines 77-514)
   - `MarkSymbolVisitor` struct - traverses HIR to mark live symbols
   - `mark_live_symbols()` method (line 321) - performs worklist-based reachability analysis
   - Tracks which items are live based on:
     - Public visibility
     - Entry points
     - Items with special attributes (`#[used]`, `#[no_mangle]`, etc.)
     - Items called from live code

2. **Allow/Expect Attribute Handling** (lines 681-716)
   - `has_allow_dead_code_or_lang_attr()` - checks for suppression attributes
   - Distinguishes between `#[allow(dead_code)]` and `#[expect(dead_code)]`

   **Important behavior** (lines 331-356):
   - `#[allow]`: Items within are treated as live, preventing warnings for the item and everything it calls
   - `#[expect]`: Properly tracks whether the lint was needed (for unfulfilled expectation warnings)

3. **Dead Code Detection** (lines 868-1136)
   - `DeadVisitor` struct - checks for dead code after live analysis
   - `check_definition()` method (line 1105) - checks individual definitions
   - `warn_dead_code()` method (line 1096) - emits warnings for dead items

4. **Module-Level Check** (lines 1138-1226)
   - `check_mod_deathness()` - entry point for checking a module
   - Checks structs, enums, functions, and their associated items
   - Groups related dead items for better diagnostics

#### Implementation Details

- Uses worklist algorithm for reachability analysis
- Handles special cases:
  - Tuple struct constructors
  - Trait implementations
  - Fields with `repr(C)` or `repr(transparent)`
  - Derived traits with `#[rustc_trivial_field_reads]`
  - Self-assignments
  - Union fields

## Tests

### Primary Test Location

**Directory**: `tests/ui/lint/dead-code/`

### Key Test Files

1. **Basic Tests**
   - `lint-dead-code-1.rs` - Basic dead code detection (structs, functions, enums, statics)
   - `lint-dead-code-2.rs` through `lint-dead-code-6.rs` - Various edge cases
   - `basic.rs` - Simple dead code examples

2. **Allow/Expect Attribute Tests**
   - `allow-or-expect-dead_code-114557.rs` - Tests `#[allow(dead_code)]` behavior
   - `allow-or-expect-dead_code-114557-2.rs` - Additional allow/expect scenarios
   - `allow-or-expect-dead_code-114557-3.rs` - More complex cases

   Example from `lint-dead-code-1.rs:108-112`:
   ```rust
   // Code with #[allow(dead_code)] should be marked live (and thus anything it
   // calls is marked live)
   #[allow(dead_code)]
   fn g() { h(); }
   fn h() {}  // Not warned - called from allowed function
   ```

3. **Specific Feature Tests**
   - `multiple-dead-codes-in-the-same-struct.rs` - Grouped diagnostics
   - `closure-bang.rs` - Dead code in closures
   - `impl-trait.rs` - Dead code with impl trait
   - `enum-variants.rs` - Unused enum variants
   - `in-closure.rs` - Dead code detection in closures
   - `issue-*.rs` - Regression tests for specific bugs

4. **Other Test Locations**
   - `tests/ui/associated-consts/associated-const-dead-code.rs` - Dead associated constants
   - `tests/ui/attributes/used/used-not-dead-code-lint.rs` - Interaction with `#[used]` attribute

### Test Coverage

The tests cover:
- Basic dead code detection (functions, structs, enums, constants, statics)
- Allow/expect attribute handling
- Nested items and closures
- Trait implementations
- Generic types
- Pattern matching usage
- Field access patterns
- Type aliases
- Associated items
- Edge cases with visibility and reachability

## Related Files

- `compiler/rustc_lint/src/lib.rs` - Lint registration
- `compiler/rustc_session/lint/builtin.rs` - Re-exports `DEAD_CODE` lint
- `compiler/rustc_hir_analysis/src/check/check.rs` - References dead code checking
