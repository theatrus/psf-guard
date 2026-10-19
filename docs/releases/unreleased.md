# Unreleased

- The upgrade notice recommends import and sync for new Target Scheduler
  databases and explains how a later update will move existing setups to a PSF
  Guard catalog. A neutral panel and accent border keep the notice prominent.
- Import settings, satellite results, planning help and collaboration messages
  explain what PSF Guard does and what you can do next. The import preview now
  states when “Skip processed files” also skips calibrated and registered copies.

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

- Short color compositions you start can run alongside stack builds when the
  prepared channels are cached, or when they need no deconvolution or RC-Astro
  and no other composition is preparing the same inputs. Color previews after
  a build you start follow the same rule once their target's channels finish.
  Other compositions keep their place in the build queue.

- With **Snap** on, dragging a mosaic panel along its row no longer pulls it
  into the next row when a panel there shares only a corner with it.

- Upgrading PSF Guard no longer stacks every channel again and rebuilds every
  calibration master when the new Seiza release leaves stacking output
  unchanged. **Build current set** reuses the existing stacks.
- A Seiza release that changes only how previews are stretched no longer
  stacks anything again. **Build current set** renders each card preview
  again from the stack's FITS. A one-shot-color stack whose color calibration
  is out of date renders again the same way; it used to need a rebuild.
- **Combine channel stacks** builds a color preview from WBPP masters in a
  card marked *WBPP*. Before, a project whose stacks were split by exposure
  showed the masters in an *Unknown exposure* card that could not be built.

- **Server information** scrolls within the window and keeps its heading and
  close controls visible. Database IDs, files and image folders have separate
  labels; long paths wrap. Server information and the database list each show
  their own loading, error and retry states.
- A queued card preview shows **Waiting to render preview**, matching the
  job list. The stack stays ready while its preview refreshes.
- Successful browser sign-ins reload the current URL to load the server's
  current UI after an update. PSF Guard checks that the session was saved first;
  failed sign-ins stay on the login form.
