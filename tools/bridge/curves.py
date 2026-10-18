"""Colour distortions between the drawn strip and the capture.

random_curve: monotonic per-channel transfer curves standing in for the WoW Gamma / Brightness /
Contrast settings, Windows HDR (SDR white level, highlight clip) and colour temperature / Night
Light style per-channel gains. Each curve maps an 8-bit drawn value to a captured value.
random_mix: grey-preserving 3x3 mixing matrices standing in for the WoW colourblind filter
(colorblindSimulator + colorblindWeaknessFactor): every row sums to 1, so greys stay grey."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import proto as P  # noqa: E402


def random_curve(rng, profile="envelope"):
    """(3, 256) float array: captured value (before noise) per channel per drawn value.
    profile 'identity' = no change, 'envelope' = the design envelope, 'harsh' = beyond it."""
    np = P._numpy()
    v = np.arange(256) / 255.0
    if profile == "identity":
        return np.tile(v * 255.0, (3, 1))
    if profile == "envelope":
        g = np.exp(rng.uniform(np.log(0.45), np.log(2.5)))
        contrast = rng.uniform(0.7, 1.4)
        offset = rng.uniform(-0.08, 0.12)
        gains = [rng.uniform(0.85, 1.0), rng.uniform(0.7, 1.0), rng.uniform(0.45, 1.0)]
        wclip = rng.uniform(0.8, 1.0)
        wig = rng.uniform(0.0, 0.3)
    else:  # harsh
        g = np.exp(rng.uniform(np.log(0.3), np.log(3.5)))
        contrast = rng.uniform(0.5, 1.8)
        offset = rng.uniform(-0.2, 0.25)
        gains = [rng.uniform(0.5, 1.0), rng.uniform(0.4, 1.0), rng.uniform(0.25, 1.0)]
        wclip = rng.uniform(0.6, 1.0)
        wig = rng.uniform(0.0, 0.6)
    out = np.empty((3, 256))
    for c in range(3):
        gc = g * rng.uniform(0.93, 1.07)
        x = np.clip((v - 0.5) * contrast + 0.5 + offset, 0.0, 1.0)
        y = x ** gc
        knots = np.concatenate([[0.0], np.cumsum(rng.uniform(0.3, 1.7, 8))])
        knots /= knots[-1]
        w = np.interp(y, np.linspace(0, 1, 9), knots)
        y = (1 - wig) * y + wig * w
        out[c] = 255.0 * np.minimum(wclip, gains[c] * y)
    return out


def random_mix(rng):
    """A grey-preserving colour mixing matrix (rows sum to 1): identity blended with a random
    row-stochastic matrix by a random weakness factor, like the colourblind simulator."""
    np = P._numpy()
    r = rng.uniform(0.0, 1.0, (3, 3)) ** 2
    r /= r.sum(axis=1, keepdims=True)
    w = rng.uniform(0.3, 1.0)
    return (1.0 - w) * np.eye(3) + w * r


def min_gap(curve):
    """Smallest captured gap between adjacent strip levels over the 3 channels."""
    np = P._numpy()
    return float(np.min(np.diff(curve[:, list(P.LEVELS)], axis=1)))
