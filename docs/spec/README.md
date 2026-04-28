# Versioned Specifications

Stable, explicitly-versioned specifications for Boruna's wire formats and language surfaces. Each entry below is a frozen contract: minor (`x.y → x.(y+1)`) bumps are forward-compatible; major (`x.y → (x+1).0`) bumps are breaking and require a migration path.

| Spec                    | Version | File                                            |
|-------------------------|---------|-------------------------------------------------|
| `.ax` language          | 1.0     | [ax-language-1.0.md](./ax-language-1.0.md)      |
| Evidence bundle format  | 1.0     | [evidence-bundle-1.0.md](./evidence-bundle-1.0.md) |
