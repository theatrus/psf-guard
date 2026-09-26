# Astronomy attribution

The experimental Director visibility module uses the unmodified `sofars`
0.6.1 crate, a Rust translation of routines and computations from the IAU
Standards of Fundamental Astronomy (SOFA) ANSIC software. Director's wrapper
validates inputs, converts units, and applies local constraints. Director is
not software supplied or endorsed by SOFA, and does not claim authorship of
the original algorithms.

The complete upstream MIT and SOFA terms are in [SOFARS-LICENSE.txt](SOFARS-LICENSE.txt).
Include both files in distributions containing this implementation. The pinned
runtime artifact includes these notices for downstream plugin packaging.

- Upstream: <https://github.com/astro-xao/sofars>
- SOFA: <https://www.iausofa.org/>
