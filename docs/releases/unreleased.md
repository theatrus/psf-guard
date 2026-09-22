# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

- **AstroBin** action on every project card and target row of the
  Overview, and an `astrobin-csv` command, write the acquisition CSV
  AstroBin's upload page imports: one row per night, filter and exposure
  length. **Essentials** is the date, filter, count and duration; **Full**
  adds binning, gain, sensor and ambient temperature, the f-number, and
  the darks, flats and bias the calibration library matches to each night.
  The dialog previews the rows and asks for the AstroBin id of any filter
  it does not know yet; **Settings → Setups → AstroBin** keeps the map.

## Changed

## Fixed
