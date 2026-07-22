//! Border extrapolation, matching OpenCV's `BorderTypes`.

/// How to extrapolate pixels outside the image, using OpenCV's naming.
///
/// For an image row `a b c d e f` the modes produce:
/// - `Replicate`:  `a a a | a b c d e f | f f f`
/// - `Reflect`:    `c b a | a b c d e f | f e d`
/// - `Reflect101`: `d c b | a b c d e f | e d c` (OpenCV's `BORDER_DEFAULT`)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderMode {
    Replicate,
    Reflect,
    Reflect101,
}

impl BorderMode {
    /// Map a possibly out-of-range coordinate into `0..len`.
    ///
    /// Handles coordinates that overshoot by more than one image width by
    /// folding repeatedly, like OpenCV's `borderInterpolate`.
    #[inline]
    pub fn map(self, mut p: isize, len: usize) -> usize {
        debug_assert!(len > 0);
        let n = len as isize;
        if p >= 0 && p < n {
            return p as usize;
        }
        match self {
            BorderMode::Replicate => p.clamp(0, n - 1) as usize,
            BorderMode::Reflect => {
                loop {
                    if p < 0 {
                        p = -p - 1;
                    } else if p >= n {
                        p = 2 * n - p - 1;
                    } else {
                        break;
                    }
                }
                p as usize
            }
            BorderMode::Reflect101 => {
                if n == 1 {
                    return 0;
                }
                loop {
                    if p < 0 {
                        p = -p;
                    } else if p >= n {
                        p = 2 * n - p - 2;
                    } else {
                        break;
                    }
                }
                p as usize
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replicate() {
        let m = BorderMode::Replicate;
        assert_eq!(m.map(-2, 5), 0);
        assert_eq!(m.map(-1, 5), 0);
        assert_eq!(m.map(0, 5), 0);
        assert_eq!(m.map(4, 5), 4);
        assert_eq!(m.map(5, 5), 4);
        assert_eq!(m.map(9, 5), 4);
    }

    #[test]
    fn reflect() {
        let m = BorderMode::Reflect;
        assert_eq!(m.map(-1, 5), 0);
        assert_eq!(m.map(-2, 5), 1);
        assert_eq!(m.map(5, 5), 4);
        assert_eq!(m.map(6, 5), 3);
    }

    #[test]
    fn reflect101() {
        let m = BorderMode::Reflect101;
        assert_eq!(m.map(-1, 5), 1);
        assert_eq!(m.map(-2, 5), 2);
        assert_eq!(m.map(5, 5), 3);
        assert_eq!(m.map(6, 5), 2);
        assert_eq!(m.map(-1, 1), 0);
    }
}
