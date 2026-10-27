import type { ReactNode } from "react";
import { useState } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { HttpResponse, http } from "msw";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { server } from "../../test/msw-server";
import AutoImportFields from "../AutoImportFields";
import ScanRulesFields from "../ScanRulesFields";
import TauriSettings from "../TauriSettings";
import {
  DEFAULT_AUTOIMPORT_SETTINGS,
  DEFAULT_SCAN_RULES,
  type AutoImportSettings,
  type ImportJobProgress,
  type ScanRules,
} from "../../api/types";

function createWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={queryClient}>
        <MemoryRouter>{children}</MemoryRouter>
      </QueryClientProvider>
    );
  };
}

const ok = <T,>(data: T) => ({
  success: true,
  data,
  error: null,
  status: "ready",
});

function RulesHarness({ onChange }: { onChange: (rules: ScanRules) => void }) {
  const [rules, setRules] = useState(DEFAULT_SCAN_RULES);
  return (
    <ScanRulesFields
      value={rules}
      onChange={(next) => {
        setRules(next);
        onChange(next);
      }}
    />
  );
}

describe("ScanRulesFields", () => {
  it("keeps one pattern per line and sets the light filters", () => {
    let latest = DEFAULT_SCAN_RULES;
    render(<RulesHarness onChange={(rules) => (latest = rules)} />);
    fireEvent.change(screen.getByLabelText("Exclude:"), {
      target: { value: "**/Hocus Focus/**\n**/PierCam/**" },
    });
    expect(latest.exclude).toEqual(["**/Hocus Focus/**", "**/PierCam/**"]);
    fireEvent.change(screen.getByLabelText("Include only:"), {
      target: { value: "*LIGHT*" },
    });
    expect(latest.include).toEqual(["*LIGHT*"]);
    fireEvent.click(
      screen.getByRole("checkbox", { name: "Require LIGHT in the image-type header" }),
    );
    fireEvent.click(
      screen.getByRole("checkbox", { name: "Lights need a target name" }),
    );
    const byPosition = screen.getByRole("checkbox", {
      name: "Match targets by position",
    });
    expect(byPosition).toBeChecked();
    fireEvent.click(byPosition);
    fireEvent.change(screen.getByLabelText("Minimum light exposure (s):"), {
      target: { value: "10" },
    });
    expect(latest).toMatchObject({
      lights_must_say_light: true,
      require_object: true,
      match_by_position: false,
      min_exposure_seconds: 10,
    });
    fireEvent.change(screen.getByLabelText("Minimum light exposure (s):"), {
      target: { value: "" },
    });
    expect(latest.min_exposure_seconds).toBeNull();
  });
});

describe("AutoImportFields", () => {
  it("asks first about new targets unless told otherwise", () => {
    let latest: AutoImportSettings = {
      ...DEFAULT_AUTOIMPORT_SETTINGS,
      enabled: true,
    };
    render(
      <AutoImportFields value={latest} onChange={(next) => (latest = next)} />,
    );
    const select = screen.getByLabelText("New targets:");
    expect(select).toHaveValue("ask");
    expect(
      Array.from((select as HTMLSelectElement).options).map(
        (option) => option.textContent,
      ),
    ).toEqual(["Ask first", "Leave out", "Create a project"]);
    fireEvent.change(select, { target: { value: "create" } });
    expect(latest.new_targets).toBe("create");
  });
});

describe("Settings scan rules and Review", () => {
  // jsdom has no scrolling; the preview panel scrolls itself into view.
  const originalScrollIntoView = Element.prototype.scrollIntoView;
  beforeEach(() => {
    Element.prototype.scrollIntoView = () => {};
  });
  afterEach(() => {
    // Unmount first: an effect still pending when the test ends would
    // otherwise scroll after the stub is gone (seen on a slow CI runner).
    cleanup();
    Element.prototype.scrollIntoView = originalScrollIntoView;
  });
  const idle: ImportJobProgress = {
    running: false,
    stage: "",
    image_dirs: [],
    total_files: 0,
    scanned_files: 0,
    outcome: null,
  };
  const held: ImportJobProgress = {
    running: false,
    stage: "complete",
    image_dirs: ["/images"],
    total_files: 4,
    scanned_files: 4,
    trigger: "automatic",
    outcome: {
      scanned: 4,
      unreadable: 0,
      non_light: 0,
      skipped_existing: 0,
      imported: 0,
      attached: 0,
      projects_created: 0,
      targets_created: 0,
      templates_created: 0,
      templates_reused: 0,
      plans_created: 0,
      profile_id: "",
      dry_run: false,
      project_summaries: [],
      attach_summaries: [],
      created_target_ids: [],
      attached_target_ids: [],
      calibration: {
        imported: 0,
        updated: 0,
        skipped_existing: 0,
        bias: 0,
        dark: 0,
        dark_flat: 0,
        flat: 0,
      },
      new_targets: "ask",
      skipped_unmatched: 4,
      unmatched: [
        {
          target: "NGC 7000",
          frames: 4,
          folders: ["/images/2026/NGC 7000/2026-10-01/LIGHT"],
        },
      ],
    },
  };

  function serveCatalog(
    saved: unknown[],
    imports: unknown[],
    preview?: ImportJobProgress,
  ) {
    // The import job reads idle until a preview starts, then reports it.
    let job: ImportJobProgress = idle;
    server.use(
      http.get("/api/info", () =>
        HttpResponse.json(
          ok({
            version: "test",
            cache_directory: "/tmp/cache",
            allow_database_management: true,
          }),
        ),
      ),
      http.get("/api/databases", () =>
        HttpResponse.json(
          ok([
            {
              id: "rig",
              name: "Rig",
              database_path: "/tmp/rig.sqlite",
              image_directories: ["/images"],
              autoimport: {
                enabled: true,
                on_open: true,
                interval_minutes: 0,
                scope: "all",
                backfill: false,
                accept_other_rigs: false,
              },
              scan_rules: {
                exclude: ["**/Hocus Focus/**"],
                include: [],
                lights_must_say_light: false,
                require_object: true,
                min_exposure_seconds: null,
              },
            },
          ]),
        ),
      ),
      http.get("/api/db/rig/import", () =>
        HttpResponse.json(ok({ started: false, progress: job })),
      ),
      http.get("/api/db/rig/import/folders", () =>
        HttpResponse.json(
          ok([{ name: "images", path: "/images", children: [] }]),
        ),
      ),
      http.get("/api/db/rig/autoimport", () =>
        HttpResponse.json(
          ok({ settings: null, progress: held, last_started_at: 1 }),
        ),
      ),
      http.post("/api/db/rig/import", async ({ request }) => {
        imports.push(await request.json());
        job = preview ?? {
          ...idle,
          running: true,
          stage: "scanning",
          image_dirs: ["/images"],
        };
        return HttpResponse.json(ok({ started: true, progress: job }));
      }),
      http.put("/api/databases/rig", async ({ request }) => {
        saved.push(await request.json());
        return HttpResponse.json(
          ok({
            id: "rig",
            name: "Rig",
            database_path: "/tmp/rig.sqlite",
            image_directories: ["/images"],
          }),
        );
      }),
    );
  }

  it("shows saved rules, asks first by default, and saves only edited rules", async () => {
    const saved: Array<{
      scan_rules?: ScanRules;
      autoimport?: AutoImportSettings;
    }> = [];
    serveCatalog(saved, []);
    render(<TauriSettings isOpen initialIntent="database-import" initialDatabaseId="rig" onClose={() => undefined} />, {
      wrapper: createWrapper(),
    });
    fireEvent.click(await screen.findByText('Scan rules · Saved'));
    expect(await screen.findByLabelText("Exclude:")).toHaveValue("**/Hocus Focus/**");
    expect(
      screen.getByRole("checkbox", { name: "Lights need a target name" }),
    ).toBeChecked();
    // Saved before the setting existed: Ask first.
    expect(screen.getByLabelText("New targets:")).toHaveValue("ask");
    fireEvent.change(screen.getByLabelText("Exclude:"), {
      target: { value: "**/Hocus Focus/**\n**/PierCam/**" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save import settings" }));
    await waitFor(() => expect(saved).toHaveLength(1));
    expect(saved[0].scan_rules?.exclude).toEqual([
      "**/Hocus Focus/**",
      "**/PierCam/**",
    ]);
    expect(saved[0].scan_rules?.require_object).toBe(true);
    // Stored rules from before the setting existed match by position.
    expect(saved[0].scan_rules?.match_by_position).toBe(true);
    expect(saved[0].autoimport).toBeUndefined();
  });

  it("says what the previewed import would change in the scheduler", async () => {
    const preview: ImportJobProgress = {
      ...held,
      trigger: "manual",
      outcome: {
        ...held.outcome!,
        dry_run: true,
        imported: 3,
        attached: 3,
        templates_created: 1,
        plans_created: 1,
        plans_raised: 2,
        new_targets: "create",
        skipped_unmatched: 0,
        unmatched: [],
        attach_summaries: [
          {
            project: "Andromeda",
            target: "M31",
            frames: 3,
            matched_by: "name",
            plans_created: 1,
            plans_raised: 2,
          },
        ],
      },
    };
    serveCatalog([], [], preview);
    render(<TauriSettings isOpen initialIntent="database-import" initialDatabaseId="rig" onClose={() => undefined} />, {
      wrapper: createWrapper(),
    });
    fireEvent.click(
      await screen.findByRole(
        "button",
        { name: "Preview import" },
        { timeout: 5000 },
      ),
    );
    // The import progress re-renders while it settles, so query again rather
    // than hold a node that a later render may replace.
    await waitFor(
      () => {
        expect(
          screen.getByText(
            /existing Andromeda \/ M31 — \+3 frame\(s\) \(name match\) · adds 1 exposure plan · raises acquired on 2 plans/,
          ),
        ).toBeInTheDocument();
        expect(
          screen.getByText("Adds 1 exposure template(s)"),
        ).toBeInTheDocument();
      },
      { timeout: 5000 },
    );
  });

  it("reviews held frames by previewing an import of their folders", async () => {
    const imports: Array<{ dry_run?: boolean; image_dirs?: string[] }> = [];
    serveCatalog([], imports);
    render(<TauriSettings isOpen initialIntent="database-import" initialDatabaseId="rig" onClose={() => undefined} />, {
      wrapper: createWrapper(),
    });
    await waitFor(
      () =>
        expect(
          screen.getByText("4 held: no matching project"),
        ).toBeInTheDocument(),
      { timeout: 5000 },
    );
    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    await waitFor(() => expect(imports).toHaveLength(1));
    expect(imports[0]).toMatchObject({
      dry_run: true,
      image_dirs: ["/images/2026/NGC 7000/2026-10-01/LIGHT"],
    });
    // The held folder sits below what the folder tree lists, so it shows
    // on its own, ticked, and can be unticked before the next preview.
    expect(
      await screen.findByRole(
        "checkbox",
        { name: "/images/2026/NGC 7000/2026-10-01/LIGHT" },
        { timeout: 5000 },
      ),
    ).toBeChecked();
  });
});
