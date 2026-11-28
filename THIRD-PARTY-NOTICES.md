# Third-party notices

Lucy contains code from the projects below, each under the MIT License. As that license asks, each project's copyright notice and permission notice follow, copied from the project's own `LICENSE` file at the revision named. [CREDITS.md](CREDITS.md) says what each project gave and where it is in this repository. Lucy's own notice is in [LICENSE](LICENSE).

## TACTLib (overtools)

Source: [`LICENSE`](https://github.com/overtools/TACTLib/blob/a00459fe0885a58da19bceb0a366605c92f1927b/LICENSE) at commit `a00459fe`. In Lucy: the CASC storage, BLTE, encoding table, bundle and content manifest readers, the build 104319 manifest key table (`crates/casc/src/manifest.rs` `KEY_TABLE`) and the static key from its `Overwatch.keyring` (`crates/casc/src/config.rs` `STATIC_KEYS`).

```text
MIT License

Copyright (c) 2020 overtools

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## OWLib (overtools)

Sources: [`LICENSE`](https://github.com/overtools/OWLib/blob/7afe5f52706e7002ed8c2717299cd60846f3fa3a/LICENSE) at commit `7afe5f52` and [`LICENSE`](https://github.com/overtools/OWLib/blob/a1023ef2c7aa9842f8764c6a7dd57c029744a313/LICENSE) at commit `a1023ef2`; the two differ only in the years. In Lucy: the STUv2 reader in `crates/stu` and its type table `crates/stu/data/types.txt`, generated from TankLib's STU types; DataTool's hero, map and unlock lists and the `GUIDNames.csv` names (`crates/assets/src/lists/mod.rs` `GUID_NAMES`); TankLib's skeleton, animation and map chunk readers in `crates/assets`.

```text
MIT License

Copyright (c) 2016-2022 overtools

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

```text
MIT License

Copyright (c) 2016-2023 overtools

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## ORPHEUS (leafall903 and contributors)

Source: [`LICENSE`](https://github.com/leafall903/ORPHEUS/blob/main/LICENSE) on branch `main` (2026-11-23). The earlier commits CREDITS.md names (`c239a84`, `b2a782e`, `2f85cee`, `1819125b`, `323b13dd`, …) carry the same license with the first copyright line only; `49aeb56f`, `c57ba58d` and `90101095` carry both lines. In Lucy: the fork's starting tree (the Rust port, the dashboard, the server integration, and the TLS relay technique that `crates/relay` is built on: the C++ relay of the imported tree, whose authors CREDITS.md thanks, came under this license with no notice of its own), and the ports CREDITS.md lists by commit.

```text
MIT License

Copyright (c) 2026 Arlecchino
Copyright (c) 2026 leafall903 and ORPHEUS contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Overwatch 1.74 Lobby Research (Boi-027)

Source: [`LICENSE`](https://github.com/Boi-027/Overwatch-1-v1.74-Lobby-Research/blob/main/LICENSE) on branch `main` (2026-11-23). In Lucy: the lobby protocol, handshake, JAM cipher and framing, and the lobby services.

```text
MIT License

Copyright (c) 2026 Arlecchino

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## overwatch-1.74-lobby-server (LizRollin)

Source: [`LICENSE`](https://github.com/LizRollin/overwatch-1.74-lobby-server/blob/9f8f3bcb5eccab6fb7738bee3c2371fcf0623f29/LICENSE) at commit `9f8f3bcb`. In Lucy: the game transport, statescript runtime and payload layouts, collision, character mover and combat, and the build data in `crates/fixed/data` that came from it.

```text
MIT License

Copyright (c) 2026 Arlecchino

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## safe_arch

Source: [`LICENSE-MIT.md`](https://docs.rs/crate/safe_arch/0.7.4/source/LICENSE-MIT.md) from safe_arch 0.7.4. The dependency offers Zlib, Apache-2.0 or MIT; Lucy uses it under MIT. In Lucy: safe x86-64 SSE wrappers in `crates/simd/src/lib.rs`, used for the mover's native reciprocal-square-root estimate in `crates/physics/src/mover/posture.rs`.

```text
MIT License

Copyright (c) 2023 Daniel "Lokathor" Gee.

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice (including the next paragraph) shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
```

## Credited without code

CREDITS.md also thanks projects whose findings, not code, Lucy uses: AipNooBest's overwatch-1.74-lobby-server (a finding about preloaded gun definitions; MIT, `Copyright (c) 2026 Arlecchino`), p3n1cillin/ow1-preservation (the HUD's team colours) and the researchers named at its end. No notice is needed for them.
