import { expect, test, type Locator, type Page } from '@playwright/test';
import { openSettings, registerFixtureDb, resetDatabases, waitForCacheReady, selectSettingsSection } from './helpers';

// Native fields draw from the dark theme's --color-input-* tokens however
// the operating system is set. While the app told the browser it could draw
// either scheme, a system in light mode got white fields on the dark pages.

let dbId: string;

test.beforeEach(async ({ request }) => {
  await resetDatabases(request);
  const entry = await registerFixtureDb(request, { name: 'Form fields e2e', slug: 'form-fields-e2e' });
  dbId = entry.id;
  await waitForCacheReady(request, dbId);
});

/** A token's value, computed the way the browser computes a colour. */
async function token(page: Page, name: string): Promise<string> {
  return page.evaluate((property) => {
    const probe = document.createElement('div');
    probe.style.color = `var(${property})`;
    document.body.appendChild(probe);
    const value = getComputedStyle(probe).color;
    probe.remove();
    return value;
  }, name);
}

async function colours(field: Locator) {
  return field.evaluate((element) => {
    const style = getComputedStyle(element);
    return {
      background: style.backgroundColor,
      text: style.color,
      border: style.borderTopColor,
      scheme: style.colorScheme,
    };
  });
}

async function expectThemed(page: Page, field: Locator) {
  const drawn = await colours(field);
  expect(drawn.background).not.toBe('rgb(255, 255, 255)');
  expect(drawn).toEqual({
    background: await token(page, '--color-input-bg'),
    text: await token(page, '--color-input-text'),
    border: await token(page, '--color-input-border'),
    scheme: 'dark',
  });
}

for (const colorScheme of ['light', 'dark'] as const) {
  test.describe(`with the system in ${colorScheme} mode`, () => {
    test.use({ colorScheme });

    test('Settings fields take the dark theme’s input colours', async ({ page }) => {
      await page.goto('/');
      await openSettings(page);
      await selectSettingsSection(page, 'calibration');
      const reach = page.getByLabel('Maximum time difference for darks', { exact: true });
      await expect(reach).toBeVisible();
      await expectThemed(page, reach);
      await expectThemed(page, page.getByLabel('Dark selection preference'));

      // An out-of-range value marks the field invalid, in the theme's colour.
      await reach.fill('0');
      await expect(reach).toHaveAttribute('aria-invalid', 'true');
      expect((await colours(reach)).border).toBe(await token(page, '--color-input-invalid-border'));
    });

    test('the Sequence Scoring limits take the dark theme’s input colours', async ({ page }) => {
      await page.goto(`/#/sequence?db=${encodeURIComponent(dbId)}&project=1&target=1`);
      await expect(page.locator('.sequence-image-card')).toHaveCount(3, { timeout: 15_000 });
      await page.locator('.scoring-penalty-control summary').click();
      const limit = page.getByLabel('Max HFR');
      await expect(limit).toBeVisible();
      await expectThemed(page, limit);

      await limit.focus();
      expect((await colours(limit)).border).toBe(await token(page, '--state-focus-border'));
    });
  });
}

for (const width of [1280, 390]) {
  test(`settings labels and help fit at ${width}px`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width, height: 900 });
    await page.goto('/');
    await openSettings(page);
    const panel = page.getByRole('tabpanel');
    const sections = [
      { id: 'calibration', label: 'Minimum darks for a single-night master' },
      { id: 'storage', label: /^(Cache v|V)olume usage limit, in percent$/ },
      { id: 'performance', label: 'Interactive work share of cores' },
      { id: 'stacking', label: 'Recent-frame requirement (days)' },
    ] as const;
    for (const section of sections) {
      await selectSettingsSection(page, section.id);
      const control = panel.getByLabel(section.label, { exact: true });
      await expect(control).toBeVisible();
      await control.scrollIntoViewIfNeeded();
      const bounds = await control.boundingBox();
      expect(bounds).not.toBeNull();
      expect(bounds!.x).toBeGreaterThanOrEqual(0);
      expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(width);
      expect(await panel.evaluate((element) => element.scrollWidth - element.clientWidth)).toBeLessThanOrEqual(1);
      await page.screenshot({ path: testInfo.outputPath(`${section.id}-${width}.png`) });
    }
    await selectSettingsSection(page, 'backups');
    await expect(panel.getByText('PSF Guard deletes older scheduled backups when the count exceeds this limit.')).toBeVisible();
    expect(await panel.evaluate((element) => element.scrollWidth - element.clientWidth)).toBeLessThanOrEqual(1);
    await page.screenshot({ path: testInfo.outputPath(`backups-${width}.png`) });
  });
}

for (const width of [1280, 390]) {
  test(`dropdown arrows have room inside the border at ${width}px`, async ({ page, browserName }, testInfo) => {
    await page.setViewportSize({ width, height: 900 });
    await page.goto('/');
    await openSettings(page);
    await selectSettingsSection(page, 'calibration');
    const preference = page.getByLabel('Dark selection preference', { exact: true });
    await expect(preference).toBeVisible();
    await preference.scrollIntoViewIfNeeded();

    // Settings override the base field's padding and background, but must
    // still leave room for the arrow and keep it inside the border.
    const geometry = await preference.evaluate((element) => {
      const style = getComputedStyle(element);
      return {
        appearance: style.appearance,
        image: style.backgroundImage,
        insets: style.backgroundPositionX.split(',').map((position) =>
          Number(/calc\(100% - ([\d.]+)px\)/.exec(position)?.[1] ?? NaN),
        ),
        repeat: style.backgroundRepeat,
        padding: parseFloat(style.paddingRight),
        sizes: style.backgroundSize.split(',').map((size) => parseFloat(size)),
      };
    });
    expect(geometry.appearance).toBe('none');
    expect(geometry.image).toContain('linear-gradient');
    expect(geometry.repeat.split(',').map((repeat) => repeat.trim())).toEqual(
      expect.arrayContaining(['no-repeat']),
    );
    expect(geometry.repeat.split(',').every((repeat) => repeat.trim() === 'no-repeat')).toBe(true);
    expect(geometry.insets).toHaveLength(2);
    for (const [index, inset] of geometry.insets.entries()) {
      expect(inset).toBeGreaterThanOrEqual(8);
      expect(inset + geometry.sizes[index % geometry.sizes.length] + 4).toBeLessThanOrEqual(geometry.padding);
    }

    const options = await preference.locator('option').evaluateAll((items) =>
      items.map((item) => (item as HTMLOptionElement).value),
    );
    await preference.selectOption(options[1]);
    await expect(preference).toHaveValue(options[1]);
    await preference.focus();
    await expect(preference).toBeFocused();
    await page.screenshot({ path: testInfo.outputPath(`dropdown-${width}.png`) });

    if (browserName === 'chromium') {
      await page.emulateMedia({ forcedColors: 'active' });
      await expect(preference).toHaveCSS('appearance', 'auto');
      await expect(preference).toHaveCSS('background-image', 'none');
    }
  });
}
