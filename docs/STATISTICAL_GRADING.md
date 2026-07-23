# Statistical grading

This older database-metric grader finds HFR and star-count outliers in values
already stored in the catalog. It does not read image pixels.

Use [Screening and sequence quality](SCREENING.md) for the current pixel-based
checks, including plate solving, photometry, occlusion, and stray light.
The `regrade` command remains useful for quick checks of stored measurements.
The `filter-rejected` command still supports these flags, but
`move-rejects` now handles rejected-file archiving.

## What it checks

The grader groups images by target and filter. Each group needs at least three
images.

### HFR and star-count outliers

- HFR outliers can reveal poor focus, seeing, or tracking.
- Star-count outliers can reveal cloud, haze, tracking errors, or field
  rotation.
- The default limit is 2 standard deviations.
- If the median and mean differ enough, the distribution check also uses
  median absolute deviation (MAD).

These checks use catalog measurements. They do not replace a fresh
**Scan Quality** run.

### Sequence cloud check

The sequence check watches for a sudden loss of detected stars or a rise in
HFR.

1. The first N images, five by default, set the rolling median.
2. Each later image is compared with that median.
3. A star-count drop or HFR rise above the limit is rejected.
4. A rejected value does not enter the baseline.
5. After `2 × N` consecutive anomalies, the grader treats the change as a new
   stable state and seeds a new baseline from the end of that run.

Star-count loss is the primary signal. HFR rise runs as a second check.

## Options

```text
--enable-statistical

--stat-hfr
--hfr-stddev <value>             # default: 2.0

--stat-stars
--star-stddev <value>            # default: 2.0

--stat-distribution
--median-shift-threshold <value> # default: 0.1

--stat-clouds
--cloud-threshold <value>        # default: 0.2
--cloud-baseline-count <n>       # default: 5
```

`--enable-statistical` enables the configured statistical checks. The
individual `--stat-*` flags let you choose checks.

## Regrade a catalog

Start with a dry run:

```bash
psf-guard regrade catalog.sqlite --dry-run \
  --enable-statistical --stat-hfr --stat-stars
```

Write the same results:

```bash
psf-guard regrade catalog.sqlite \
  --enable-statistical --stat-hfr --stat-stars
```

Run only the sequence cloud check:

```bash
psf-guard regrade catalog.sqlite --dry-run \
  --enable-statistical --stat-clouds --cloud-threshold 0.15
```

Limit the work to one target:

```bash
psf-guard regrade catalog.sqlite --dry-run \
  --target "M31" \
  --enable-statistical --stat-hfr --stat-stars --stat-clouds
```

Automatic rejection reasons start with `[Auto]`, such as:

```text
[Auto] Statistical HFR - HFR 3.456 is 2.5σ from mean 2.890
[Auto] Cloud Detection (Stars) - Star count 210 is 35% below baseline 323
```

### Reset existing grades

`--reset` accepts:

- `none` — keep all grades and add new rejections
- `automatic` — reset grades whose reason starts with `[Auto]`
- `all` — reset every matching image to pending

The default date range is 90 days. Use `--days`, `--project`, or `--target` to
narrow it.

```bash
psf-guard regrade catalog.sqlite --dry-run \
  --reset automatic --days 30

psf-guard regrade catalog.sqlite \
  --reset all --target "NGC 7000" --days 7
```

Resets and new rejections commit in one transaction.

## Legacy file-moving command

`filter-rejected` can still combine this grading pass with its old file-moving
workflow:

```bash
psf-guard filter-rejected catalog.sqlite ./images --dry-run \
  --enable-statistical --stat-hfr --stat-stars
```

Use it only for an existing workflow that depends on its old layout. For new
archive work, grade first and use `move-rejects --db <slug>`.

## Tuning

- Begin with `--dry-run`.
- Raise a standard-deviation limit to reject fewer images.
- Lower it to reject more.
- Raise `--cloud-threshold` to ignore smaller changes.
- A larger baseline changes more slowly.
- Review several target and filter groups before adopting one set of limits.

## Performance

- The command loads the matching catalog rows and metadata into memory. It does
  not decode FITS pixels.
- Large catalogs take longer to query, sort, and group.
- Use `--project`, `--target`, or `--days` to narrow the work.
