// Empty placeholder — Plan 4 fills this in (spec §14.7 `aura support-bundle`).
//
// The CLI binary is named `aura-support` (not `aura`) because on default
// case-insensitive APFS volumes a product named `aura` would overwrite the
// `Aura` app binary in `.build/<cfg>/`. Plan 4's task F.1 may rename or split
// into a sub-package to honor the spec's literal `aura support-bundle` command.
//
// Plan 4 amendments backlog (line 52) expects this empty target to already
// exist after Plan 1's amendment A. This file satisfies that contract.

import Foundation
import Aura

print("aura-support: placeholder CLI — Plan 4 fills this in (\(AuraVersion.current))")
