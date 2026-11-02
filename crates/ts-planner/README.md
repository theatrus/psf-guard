# Target Scheduler planner emulation

`psf-guard-ts-planner` predicts what N.I.N.A. Target Scheduler will shoot on a
rig over a night, so PSF Guard's night plans can show a Target Scheduler rig's
schedule. It reads nothing itself: the server reads the rig's database into a
`Snapshot` and passes it in.

## Licence

This crate follows the planner of the Target Scheduler plugin
(<https://github.com/tcpalmer/nina.plugin.targetscheduler>) closely, so it is
under the **Mozilla Public License 2.0**, like Target Scheduler, file by file:
see [`LICENSE`](./LICENSE) and the notice at the top of each file. MPL-2.0 is a
file-level licence. The rest of PSF Guard stays Apache-2.0 and is unaffected,
as for the vendored schema in `src/ts_schema`.

## Fidelity

The crate names the Target Scheduler version it follows
(`TARGET_SCHEDULER_VERSION`) and the database schema it reads
(`SCHEMA_VERSION`). Its tests compare its choices with Target Scheduler's own
Scheduler Preview on fixed databases and start times. A Target Scheduler
release that changes planning needs new fixtures and a new version.
