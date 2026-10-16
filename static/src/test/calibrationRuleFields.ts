import type { CalibrationRuleField } from '../api/types';

/**
 * The tunable calibration checks as `GET /api/settings/calibration` sends
 * them, for component tests. Copied from the server's
 * `calibration::rules::fields`; the server is the authority.
 */
export const CALIBRATION_RULE_FIELDS: CalibrationRuleField[] = [
  {
    "key": "rotation_tolerance_deg",
    "group": "matching",
    "label": "Flat rotation tolerance",
    "help": "How far a flat's rotator angle may sit from the light it corrects. Wider accepts a rotator that re-homes loosely; one master still takes flats within 2° of each other.",
    "effect": "matching",
    "kind": {
      "type": "number",
      "min": 0.0,
      "max": 180.0,
      "step": 0.1,
      "unit": "°"
    },
    "default": 2.0
  },
  {
    "key": "focal_length_tolerance_mm",
    "group": "matching",
    "label": "Flat focal length tolerance",
    "help": "How far a flat's recorded focal length may sit from the light's, as when a profile's focal length was refined. One master still takes flats within 1 mm of each other.",
    "effect": "matching",
    "kind": {
      "type": "number",
      "min": 0.0,
      "max": 100.0,
      "step": 0.1,
      "unit": "mm"
    },
    "default": 1.0
  },
  {
    "key": "dark_temperature_tolerance_c",
    "group": "matching",
    "label": "Dark temperature tolerance",
    "help": "How far a dark's sensor temperature may sit from the light's, and a dark flat's from its flat's. Wider lets a rig that cannot hold its set point use darks shot a little warmer or cooler; one master still takes darks within 1 °C of each other.",
    "effect": "matching",
    "kind": {
      "type": "number",
      "min": 0.0,
      "max": 20.0,
      "step": 0.1,
      "unit": "°C"
    },
    "default": 3.0
  },
  {
    "key": "flat_session_hours",
    "group": "matching",
    "label": "Flat session",
    "help": "How far apart the flats one master takes may have been shot. Longer pools sky flats shot on several evenings.",
    "effect": "matching",
    "kind": {
      "type": "number",
      "min": 1.0,
      "max": 720.0,
      "step": 1.0,
      "unit": "h"
    },
    "default": 24.0
  },
  {
    "key": "use_darks",
    "group": "matching",
    "label": "Use darks",
    "help": "Off for a rig that cannot shoot matching darks: its lights take bias and flats only, and are not flagged for lacking a dark.",
    "effect": "matching",
    "kind": {
      "type": "toggle"
    },
    "default": true
  },
  {
    "key": "external_masters",
    "group": "matching",
    "label": "Masters from other software",
    "help": "A master from PixInsight, Siril or another tool is matched on what its header kept and used as it is.",
    "effect": "matching",
    "kind": {
      "type": "choice",
      "options": [
        {
          "value": "prefer",
          "label": "Use them whenever one matches"
        },
        {
          "value": "fallback",
          "label": "Only when raw frames cannot build one"
        },
        {
          "value": "ignore",
          "label": "Never"
        }
      ]
    },
    "default": "prefer"
  },
  {
    "key": "night_zone",
    "group": "matching",
    "label": "Night starts at noon in",
    "help": "Calibration splits a rig's frames into nights at local noon, so a dusk or dawn flat run stays one session. Automatic takes noon from the site longitude its frames record (the N.I.N.A. profile's), else from its computer's clock, else from when the catalog's lights are quiet. Pick the rig's standard offset when its frames record no site or a wrong one; daylight saving moves noon by an hour, which no night notices.",
    "effect": "grades",
    "kind": {
      "type": "choice",
      "options": [
        {
          "value": "auto",
          "label": "From its frames' site"
        },
        {
          "value": "-12",
          "label": "UTC−12"
        },
        {
          "value": "-11",
          "label": "UTC−11"
        },
        {
          "value": "-10",
          "label": "UTC−10"
        },
        {
          "value": "-9.5",
          "label": "UTC−9:30"
        },
        {
          "value": "-9",
          "label": "UTC−9"
        },
        {
          "value": "-8",
          "label": "UTC−8"
        },
        {
          "value": "-7",
          "label": "UTC−7"
        },
        {
          "value": "-6",
          "label": "UTC−6"
        },
        {
          "value": "-5",
          "label": "UTC−5"
        },
        {
          "value": "-4",
          "label": "UTC−4"
        },
        {
          "value": "-3.5",
          "label": "UTC−3:30"
        },
        {
          "value": "-3",
          "label": "UTC−3"
        },
        {
          "value": "-2",
          "label": "UTC−2"
        },
        {
          "value": "-1",
          "label": "UTC−1"
        },
        {
          "value": "0",
          "label": "UTC"
        },
        {
          "value": "1",
          "label": "UTC+1"
        },
        {
          "value": "2",
          "label": "UTC+2"
        },
        {
          "value": "3",
          "label": "UTC+3"
        },
        {
          "value": "3.5",
          "label": "UTC+3:30"
        },
        {
          "value": "4",
          "label": "UTC+4"
        },
        {
          "value": "4.5",
          "label": "UTC+4:30"
        },
        {
          "value": "5",
          "label": "UTC+5"
        },
        {
          "value": "5.5",
          "label": "UTC+5:30"
        },
        {
          "value": "5.75",
          "label": "UTC+5:45"
        },
        {
          "value": "6",
          "label": "UTC+6"
        },
        {
          "value": "6.5",
          "label": "UTC+6:30"
        },
        {
          "value": "7",
          "label": "UTC+7"
        },
        {
          "value": "8",
          "label": "UTC+8"
        },
        {
          "value": "8.75",
          "label": "UTC+8:45"
        },
        {
          "value": "9",
          "label": "UTC+9"
        },
        {
          "value": "9.5",
          "label": "UTC+9:30"
        },
        {
          "value": "10",
          "label": "UTC+10"
        },
        {
          "value": "10.5",
          "label": "UTC+10:30"
        },
        {
          "value": "11",
          "label": "UTC+11"
        },
        {
          "value": "12",
          "label": "UTC+12"
        },
        {
          "value": "12.75",
          "label": "UTC+12:45"
        },
        {
          "value": "13",
          "label": "UTC+13"
        },
        {
          "value": "14",
          "label": "UTC+14"
        }
      ]
    },
    "default": "auto"
  },
  {
    "key": "dark_reach_days",
    "group": "darks",
    "label": "Dark reach",
    "help": "Darks shot further than this from the light do not match. A dark marked to serve lights before or after it follows its mark instead.",
    "effect": "matching",
    "kind": {
      "type": "number",
      "min": 1.0,
      "max": 3650.0,
      "step": 1.0,
      "unit": "days"
    },
    "default": 183.0
  },
  {
    "key": "complete_dark_frames",
    "group": "darks",
    "label": "Complete night",
    "help": "When the nearest night alone has this many matching darks, the master uses that night only; with fewer, the nights within reach pool.",
    "effect": "matching",
    "kind": {
      "type": "count",
      "min": 2,
      "max": 64,
      "unit": "darks"
    },
    "default": 10
  },
  {
    "key": "dark_choice",
    "group": "darks",
    "label": "Dark choice",
    "help": "Complete set first: a complete set beats a nearer, shorter one. Bias masters and WBPP exports follow it too.",
    "effect": "matching",
    "kind": {
      "type": "choice",
      "options": [
        {
          "value": "complete",
          "label": "Complete set first"
        },
        {
          "value": "nearest",
          "label": "Nearest set"
        }
      ]
    },
    "default": "complete"
  },
  {
    "key": "flat_grouping",
    "group": "flat_groups",
    "label": "Group flats",
    "help": "Measured: nights mix in one master only when their flats measure alike, and a light takes flats from its own side of every measured change, such as new dust. By time: one master takes the flats within the flat session.",
    "effect": "matching",
    "kind": {
      "type": "choice",
      "options": [
        {
          "value": "measured",
          "label": "By measurement"
        },
        {
          "value": "time",
          "label": "By time only"
        }
      ]
    },
    "default": "measured"
  },
  {
    "key": "flat_split_dust_pct",
    "group": "flat_groups",
    "label": "Dust that splits flats",
    "help": "The largest dust difference two nights' flats may have and still share a master: what moved, new or vanished dust would leave in a light calibrated with the other night's flats, in percent of the sky background.",
    "effect": "matching",
    "kind": {
      "type": "number",
      "min": 0.1,
      "max": 10.0,
      "step": 0.1,
      "unit": "%"
    },
    "default": 1.0
  },
  {
    "key": "flat_split_pattern_pct",
    "group": "flat_groups",
    "label": "Light pattern that splits flats",
    "help": "The largest broad difference two nights' flats may have, such as a changed vignette, after a straight gradient is removed, in percent of the sky background.",
    "effect": "matching",
    "kind": {
      "type": "number",
      "min": 0.2,
      "max": 20.0,
      "step": 0.1,
      "unit": "%"
    },
    "default": 2.0
  },
  {
    "key": "flat_max_age_days",
    "group": "checks",
    "label": "Flat age limit",
    "help": "Lights whose flats were shot further away than this can't be calibrated.",
    "effect": "flags",
    "kind": {
      "type": "number",
      "min": 0.0,
      "max": 3650.0,
      "step": 1.0,
      "unit": "days",
      "zero": "no limit"
    },
    "default": 60.0
  },
  {
    "key": "flat_check_age_days",
    "group": "checks",
    "label": "Flats worth a look after",
    "help": "Lights whose flats were shot further away than this, but within the age limit, are worth a look.",
    "effect": "flags",
    "kind": {
      "type": "number",
      "min": 0.0,
      "max": 3650.0,
      "step": 1.0,
      "unit": "days",
      "zero": "off"
    },
    "default": 30.0
  },
  {
    "key": "thin_master_frames",
    "group": "checks",
    "label": "Thin master below",
    "help": "A bias or dark master built from fewer frames puts its own read noise into every light.",
    "effect": "flags",
    "kind": {
      "type": "count",
      "min": 2,
      "max": 64,
      "unit": "frames"
    },
    "default": 5
  },
  {
    "key": "check_no_library",
    "group": "checks",
    "label": "No frames for its camera",
    "help": "Nothing in the library matches the light's camera and settings, though the library holds frames for other cameras.",
    "effect": "flags",
    "kind": {
      "type": "check",
      "levels": [
        "poor"
      ]
    },
    "default": "flag"
  },
  {
    "key": "check_no_flat",
    "group": "checks",
    "label": "No matching flat",
    "help": "No flat matches the light, though the library holds flats for its camera.",
    "effect": "flags",
    "kind": {
      "type": "check",
      "levels": [
        "poor"
      ]
    },
    "default": "flag"
  },
  {
    "key": "check_no_bias_or_dark",
    "group": "checks",
    "label": "No matching bias or dark",
    "help": "Neither a bias nor a dark matches the light, though the library holds one.",
    "effect": "flags",
    "kind": {
      "type": "check",
      "levels": [
        "poor"
      ]
    },
    "default": "flag"
  },
  {
    "key": "check_stale_flats",
    "group": "checks",
    "label": "Flats past the age limit",
    "help": "The flats that match were shot further from the light than the flat age limit.",
    "effect": "flags",
    "kind": {
      "type": "check",
      "levels": [
        "poor"
      ]
    },
    "default": "flag"
  },
  {
    "key": "check_thin_dark",
    "group": "checks",
    "label": "Thin dark master",
    "help": "Its dark master would use fewer frames than the thin limit.",
    "effect": "flags",
    "kind": {
      "type": "check",
      "levels": [
        "poor"
      ]
    },
    "default": "flag"
  },
  {
    "key": "check_thin_bias",
    "group": "checks",
    "label": "Thin bias master",
    "help": "Its bias master would use fewer frames than the thin limit: poor when no dark serves the light, check when one does.",
    "effect": "flags",
    "kind": {
      "type": "check",
      "levels": [
        "poor",
        "check"
      ]
    },
    "default": "flag"
  },
  {
    "key": "check_flat_spots",
    "group": "checks",
    "label": "Changing spots in flats",
    "help": "Spots that come and go across its flats, such as drying dew, sit in most of them and so in their master. The flat-grade limits for spots carried into the master decide check or poor.",
    "effect": "flags",
    "kind": {
      "type": "check",
      "levels": [
        "poor",
        "check"
      ]
    },
    "default": "flag"
  },
  {
    "key": "check_old_flats",
    "group": "checks",
    "label": "Old flats",
    "help": "Its flats were shot further away than the 'worth a look' age.",
    "effect": "flags",
    "kind": {
      "type": "check",
      "levels": [
        "check"
      ]
    },
    "default": "flag"
  },
  {
    "key": "check_mixed_flats",
    "group": "checks",
    "label": "Flats from several nights",
    "help": "Its flat master takes flats from nights not measured against each other. Nights whose flats measure alike mix without a word.",
    "effect": "flags",
    "kind": {
      "type": "check",
      "levels": [
        "check"
      ]
    },
    "default": "flag"
  },
  {
    "key": "check_flats_differ",
    "group": "checks",
    "label": "Flats from across a change",
    "help": "A measured change, such as new dust, lies between the light and its flats, or the light was shot while it happened and nothing tells which side it is on.",
    "effect": "flags",
    "kind": {
      "type": "check",
      "levels": [
        "check"
      ]
    },
    "default": "flag"
  },
  {
    "key": "check_suspect_frames",
    "group": "checks",
    "label": "Suspect frames in its masters",
    "help": "Its masters use frames graded suspect.",
    "effect": "flags",
    "kind": {
      "type": "check",
      "levels": [
        "check"
      ]
    },
    "default": "flag"
  },
  {
    "key": "check_short_after_keep_out",
    "group": "checks",
    "label": "Short after keeping frames out",
    "help": "Frames kept out by hand leave its bias, darks or flats under the thin limit.",
    "effect": "flags",
    "kind": {
      "type": "check",
      "levels": [
        "check"
      ]
    },
    "default": "flag"
  },
  {
    "key": "grade_bias_level",
    "group": "grades",
    "label": "Bias level",
    "help": "Distance of a bias's level from its set's, in the set's noise.",
    "effect": "grades",
    "kind": {
      "type": "limits",
      "min": 0.0,
      "max": 1000.0,
      "step": 0.1,
      "unit": "σ",
      "percent": false,
      "lower_is_worse": false
    },
    "default": {
      "poor": 3.0,
      "suspect": 1.0
    }
  },
  {
    "key": "grade_bias_noise",
    "group": "grades",
    "label": "Bias noise",
    "help": "A bias's noise as a ratio to its set's.",
    "effect": "grades",
    "kind": {
      "type": "limits",
      "min": 0.0,
      "max": 100.0,
      "step": 0.05,
      "unit": "×",
      "percent": false,
      "lower_is_worse": false
    },
    "default": {
      "poor": 1.3,
      "suspect": 1.1
    }
  },
  {
    "key": "grade_bias_banding",
    "group": "grades",
    "label": "Bias banding",
    "help": "Row or column banding as a ratio to the frame's own noise.",
    "effect": "grades",
    "kind": {
      "type": "limits",
      "min": 0.0,
      "max": 100.0,
      "step": 0.05,
      "unit": "×",
      "percent": false,
      "lower_is_worse": false
    },
    "default": {
      "poor": 0.5,
      "suspect": 0.25
    }
  },
  {
    "key": "grade_dark_level",
    "group": "grades",
    "label": "Dark stray light",
    "help": "How far a dark's level sits above the quietest dark that could stand in for it, in that dark's spread. Light reaching the sensor raises it.",
    "effect": "grades",
    "kind": {
      "type": "limits",
      "min": 0.0,
      "max": 1000.0,
      "step": 0.1,
      "unit": "σ",
      "percent": false,
      "lower_is_worse": false
    },
    "default": {
      "poor": 1.0,
      "suspect": 0.5
    }
  },
  {
    "key": "grade_dark_noise",
    "group": "grades",
    "label": "Dark noise",
    "help": "A dark's noise as a ratio to its peers'.",
    "effect": "grades",
    "kind": {
      "type": "limits",
      "min": 0.0,
      "max": 100.0,
      "step": 0.05,
      "unit": "×",
      "percent": false,
      "lower_is_worse": false
    },
    "default": {
      "poor": 1.3,
      "suspect": 1.1
    }
  },
  {
    "key": "grade_dark_hot_pixels",
    "group": "grades",
    "label": "Dark hot pixels",
    "help": "A dark's hot pixels as a ratio to its peers'.",
    "effect": "grades",
    "kind": {
      "type": "limits",
      "min": 0.0,
      "max": 1000.0,
      "step": 0.1,
      "unit": "×",
      "percent": false,
      "lower_is_worse": false
    },
    "default": {
      "poor": 3.0,
      "suspect": 1.5
    }
  },
  {
    "key": "grade_dark_banding",
    "group": "grades",
    "label": "Dark banding",
    "help": "Row or column banding as a ratio to the frame's own noise.",
    "effect": "grades",
    "kind": {
      "type": "limits",
      "min": 0.0,
      "max": 100.0,
      "step": 0.05,
      "unit": "×",
      "percent": false,
      "lower_is_worse": false
    },
    "default": {
      "poor": 0.5,
      "suspect": 0.25
    }
  },
  {
    "key": "grade_flat_clipped",
    "group": "grades",
    "label": "Flat clipping",
    "help": "Share of a flat's samples at full scale.",
    "effect": "grades",
    "kind": {
      "type": "limits",
      "min": 0.0,
      "max": 1.0,
      "step": 0.0001,
      "unit": "%",
      "percent": true,
      "lower_is_worse": false
    },
    "default": {
      "poor": 0.005,
      "suspect": 0.0005
    }
  },
  {
    "key": "grade_flat_bright",
    "group": "grades",
    "label": "Flat too bright",
    "help": "The brightest channel's level as a share of full scale.",
    "effect": "grades",
    "kind": {
      "type": "limits",
      "min": 0.0,
      "max": 1.0,
      "step": 0.01,
      "unit": "%",
      "percent": true,
      "lower_is_worse": false
    },
    "default": {
      "poor": 0.92,
      "suspect": 0.8
    }
  },
  {
    "key": "grade_flat_dim",
    "group": "grades",
    "label": "Flat too dim",
    "help": "The dimmest channel's level as a share of full scale; lower is worse.",
    "effect": "grades",
    "kind": {
      "type": "limits",
      "min": 0.0,
      "max": 1.0,
      "step": 0.01,
      "unit": "%",
      "percent": true,
      "lower_is_worse": true
    },
    "default": {
      "poor": 0.03,
      "suspect": 0.1
    }
  },
  {
    "key": "grade_flat_illumination",
    "group": "grades",
    "label": "Flat light pattern",
    "help": "How far a flat's light pattern falls off its session's. Sky flats shot as the sky changes differ more.",
    "effect": "grades",
    "kind": {
      "type": "limits",
      "min": 0.0,
      "max": 1.0,
      "step": 0.001,
      "unit": "%",
      "percent": true,
      "lower_is_worse": false
    },
    "default": {
      "poor": 0.05,
      "suspect": 0.02
    }
  },
  {
    "key": "grade_flat_spots",
    "group": "grades",
    "label": "Flat spots",
    "help": "Clear spots in one flat.",
    "effect": "grades",
    "kind": {
      "type": "limits",
      "min": 0.0,
      "max": 100000.0,
      "step": 1.0,
      "unit": "spots",
      "percent": false,
      "lower_is_worse": false
    },
    "default": {
      "poor": 500.0,
      "suspect": 100.0
    }
  },
  {
    "key": "grade_flat_changing_spots",
    "group": "grades",
    "label": "Flat changing spots",
    "help": "Spots in one flat that change across its session.",
    "effect": "grades",
    "kind": {
      "type": "limits",
      "min": 0.0,
      "max": 100000.0,
      "step": 1.0,
      "unit": "spots",
      "percent": false,
      "lower_is_worse": false
    },
    "default": {
      "poor": 20.0,
      "suspect": 3.0
    }
  },
  {
    "key": "grade_flat_carried_spots",
    "group": "grades",
    "label": "Spots carried into a flat master",
    "help": "Changing spots in most of a set's flats, which its master keeps. Decides the 'changing spots in flats' light check.",
    "effect": "grades",
    "kind": {
      "type": "limits",
      "min": 0.0,
      "max": 100000.0,
      "step": 1.0,
      "unit": "spots",
      "percent": false,
      "lower_is_worse": false
    },
    "default": {
      "poor": 10.0,
      "suspect": 1.0
    }
  }
];
