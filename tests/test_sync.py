"""Tests for sync-spotify.py helpers."""
import importlib.util
from pathlib import Path

# Load the script as a module without executing the if __name__ == "__main__" block.
_spec = importlib.util.spec_from_file_location(
    "sync_spotify",
    Path(__file__).parent.parent / "scripts" / "sync-spotify.py",
)
_mod = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_mod)  # type: ignore[union-attr]

similar = _mod.similar
MATCH_THRESHOLD = _mod.MATCH_THRESHOLD


def test_pmedia_suffix_matches():
    """A (PMEDIA) tag must not drop the score below the match threshold."""
    assert similar("United in Grief (PMEDIA)", "United in Grief") >= MATCH_THRESHOLD


def test_short_prefix_does_not_match_unrelated_title():
    """'Red' must not match 'Red Dead Redemption' just because it is a prefix."""
    assert similar("Red", "Red Dead Redemption") < MATCH_THRESHOLD


def test_feat_on_one_side_pmedia_on_other():
    """Same song tagged '(feat. Drake)' on one side and '(PMEDIA)' on the other must match."""
    assert similar("One More Night (feat. Drake)", "One More Night (PMEDIA)") >= MATCH_THRESHOLD
