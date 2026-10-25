# Heart and Soul mosaic examples

Open [index.html](index.html) to compare two versions of the four-panel field.
The page works offline. The JPEGs are 1600 pixels wide; the Dropbox copy also
includes original-size PNGs in `full-size/` and completed job reports in `reports/`.

| Example | Processing |
| --- | --- |
| [H-alpha mosaic](images/ha.jpg) | Four panels on one canvas; automatic display stretch. |
| [RGB](images/rgb.jpg) | R, G and B mosaics with automatic background extraction and display stretch. |
| [SHO · shared Ha stretch](images/sho-shared.jpg) | Shared H-alpha stretch, original narrowband stars, balance off. |
| [SHO · balanced](images/sho-balanced.jpg) | Shared H-alpha stretch and OIII/SII balance; original narrowband stars. |
| [HOO · balanced](images/hoo-balanced.jpg) | Shared H-alpha stretch and OIII balance; original narrowband stars. |
| [SHO · RGB stars](images/sho-rgb-stars.jpg) | Balanced SHO nebula with RGB stars; paired star stretch. |
| [HOO · RGB stars](images/hoo-rgb-stars.jpg) | Balanced HOO nebula with RGB stars; paired star stretch. |
| [SHO · Automatic stars](images/sho-automatic-stars.jpg) | Automatic selects RGB stars from the available channels. |
| [SHO · starless](images/sho-starless.jpg) | Balanced SHO with stars removed through StarXTerminator. |
| [SHO · broadband structure](images/sho-rgb-structure.jpg) | Balanced SHO with broadband structure amount 0.25 and RGB stars; continuum subtraction off. |
| [SHO · RGB star stretch 3](images/sho-star-stretch-3.jpg) | Balanced SHO with an independent linear RGB star stretch of 3. |
| [SHO · RGB star stretch 5](images/sho-star-stretch-5.jpg) | Balanced SHO with an independent linear RGB star stretch of 5. |

![SHO with RGB stars](images/sho-rgb-stars.jpg)

![HOO with RGB stars](images/hoo-rgb-stars.jpg)

## Sources

Four existing PSF Guard panel stacks per filter were mean-binned **4× per axis**
with matching FITS/WCS changes, then integrated on Seiza's expanded canvas with
local background normalization. All four panels contributed to each channel.
The panel integrations were reused; this refresh did not rebuild the raw frames.

| Filter | Accepted source exposures | Source integration | Source calibration |
| --- | ---: | ---: | --- |
| R | 240 | 40 min | none |
| G | 240 | 40 min | none |
| B | 240 | 40 min | none |
| HA | 163 | 13 h 35 min | applied |
| OIII | 180 | 15 h 0 min | applied |
| SII | 180 | 15 h 0 min | applied |

The sources total **1,243 accepted exposures and 45 h 35 min** across six filters.
There is no separate luminance channel, so this gallery has no LRGB example.
[examples.json](examples.json) records source jobs, calibration states, fresh
channel checksums, image dimensions, processing requests and output checksums.

## Processing

The six channel mosaics and eleven color jobs use PSF Guard `50e2d76f`, published
Seiza **0.44.0** and stacking **0.51.0**, including the shared mosaic background
matcher. No local dependency overrides were used.

Color jobs ran through the tracked-job API against an isolated fixture. They
use automatic background extraction, an Auto MTF display stretch with target
median 0.2 and shadows clip −2.8, and an inscribed common-coverage crop. Each
example keeps its own crop; dimensions differ between combinations.

Narrowband balance uses shadow protection 1 and OIII/SII boosts 1. RGB-star
and starless cases use local, licensed StarXTerminator on the GPU through
RC-Astro CLI **2.6.10**. BlurXTerminator and NoiseXTerminator were not enabled.
Automatic stars resolves to RGB. The explicit stretch 3 and 5 examples stretch
the linear RGB star layer independently; the other RGB-star cases retain the
paired stretch. Gaia color calibration was skipped because the fixture has
no fresh reference solve.

The broadband structure example uses amount 0.25 with continuum subtraction
**Off**. It demonstrates that combination, not a measured continuum fit.
Background matching reduces panel seams; this field is not a seam-free
reference or a controlled comparison of calibration methods.

The source catalog and original panel stacks were not modified. The Dropbox
copy includes all requests, completed jobs and source provenance, alongside
the earlier **Seiza seam comparison** folder.
