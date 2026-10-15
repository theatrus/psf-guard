import { expect, test, type Locator, type Page } from '@playwright/test';
import { openSettings, registerFixtureDb, resetDatabases, waitForCacheReady } from './helpers';

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
      await page.getByRole('tab', { name: 'Calibration', exact: true }).click();
      const reach = page.getByLabel('Dark reach', { exact: true });
      await expect(reach).toBeVisible();
      await expectThemed(page, reach);
      await expectThemed(page, page.getByLabel('Dark choice'));

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
