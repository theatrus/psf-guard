import { expect, test, type Page } from '@playwright/test';
import { ACCOUNT_PASSWORD, startAccountsServer, type AccountsServer } from './accounts-server';
import {
  openSettings,
  registerFixtureDb,
  resetDatabases,
  waitForCacheReady, selectSettingsSection } from './helpers';

// Sequence's scoring defaults: one copy on the server, set in Settings,
// applied by every view, and changed per view in the toolbar until saved.
// The fixture's Alpha sequence
// has HFRs 2.4 / 2.5 / 2.6 and star counts 520 / 510 / 500, so a Max HFR
// of 2.55 or a Min stars of 505 flags the third frame, and Min stars 515
// the last two.

const flagged = (page: Page) => page.locator('.sequence-image-card.below-threshold');
const summary = (page: Page) => page.locator('.scoring-penalty-control summary');

async function openSequence(page: Page, url: string) {
  await page.goto(url);
  await expect(page.locator('.sequence-image-card')).toHaveCount(3, { timeout: 30_000 });
}

/** Open the Scoring popover, if it is closed. */
async function openScoring(page: Page) {
  const control = page.locator('.scoring-penalty-control');
  if ((await control.getAttribute('open')) === null) await summary(page).click();
}

async function openScoringDefaults(page: Page) {
  await openSettings(page);
  await selectSettingsSection(page, 'review');
  return page.getByRole('region', { name: 'Sequence scoring' });
}

async function closeSettings(page: Page) {
  await page.locator('.tauri-settings .close-button').click();
  await expect(page.getByRole('heading', { name: 'PSF Guard Settings' })).toHaveCount(0);
}

const BUILT_IN = {
  penalty_satellite: 1,
  penalty_pointing: 1,
  penalty_temporal: 1,
  penalty_hfr_rise: 1,
  penalty_star_loss: 1,
  hfr_reject_above: null,
  star_count_reject_below: null,
};

test.describe('without accounts', () => {
  let dbId: string;

  // The server keeps the defaults; other specs score with the built-in ones.
  test.afterEach(async ({ request }) => {
    await request.put('/api/settings/scoring', { data: BUILT_IN });
  });

  test.beforeEach(async ({ request }) => {
    await resetDatabases(request);
    const entry = await registerFixtureDb(request, {
      name: 'Imaging Rig e2e',
      slug: 'imaging-rig-e2e',
    });
    dbId = entry.id;
    await waitForCacheReady(request, dbId);
  });

  test('defaults set in Settings apply in Sequence, and the toolbar saves its own', async ({ page }, testInfo) => {
    const sequenceUrl = `/#/sequence?db=${encodeURIComponent(dbId)}&project=1&target=1`;
    await openSequence(page, sequenceUrl);
    await expect(flagged(page)).toHaveCount(0);

    // Settings › Review › Sequence scoring.
    const defaults = await openScoringDefaults(page);
    await expect(defaults.getByRole('spinbutton', { name: 'Satellite trails (percent)' }))
      .toHaveValue('100');
    await defaults.getByRole('spinbutton', { name: 'Max HFR (pixels)' }).fill('2.55');
    await defaults.getByRole('button', { name: 'Save defaults' }).click();
    await expect(defaults.getByRole('button', { name: 'Save defaults' })).toBeDisabled();
    await defaults.getByRole('heading', { name: 'Sequence scoring' }).scrollIntoViewIfNeeded();
    await page.screenshot({ path: testInfo.outputPath('scoring-defaults-settings.png') });
    await closeSettings(page);

    // The view takes them, and does not count as changed.
    await expect(flagged(page)).toHaveCount(1, { timeout: 15_000 });
    await expect(summary(page)).toHaveText('Scoring');
    await openScoring(page);
    await expect(page.getByLabel('Max HFR')).toHaveValue('2.55');
    expect(page.url()).not.toContain('maxHfr');

    // A change in the toolbar applies to this view, in its address.
    const minStars = page.getByLabel('Min stars');
    await minStars.fill('515');
    await minStars.press('Enter');
    await expect(flagged(page)).toHaveCount(2, { timeout: 15_000 });
    await expect(summary(page)).toHaveText('Scoring (changed)');
    await expect(page).toHaveURL(/minStars=515/);
    await expect(page.locator('.scoring-penalty-control')).toHaveClass(/is-set/);

    // Save as default: the view now matches, and the address is clean.
    await page.getByRole('button', { name: 'Save as default' }).click();
    await expect(summary(page)).toHaveText('Scoring');
    await expect(page).not.toHaveURL(/minStars/);
    await expect(page.getByRole('button', { name: 'Save as default' })).toBeDisabled();

    // A reload, and a link that names nothing, start from the new defaults.
    await page.reload();
    await expect(page.locator('.sequence-image-card')).toHaveCount(3, { timeout: 30_000 });
    await expect(flagged(page)).toHaveCount(2, { timeout: 15_000 });
    await expect(summary(page)).toHaveText('Scoring');

    // A link that names a value wins over the defaults.
    await openSequence(page, `${sequenceUrl}&minStars=off&maxHfr=off`);
    await expect(flagged(page)).toHaveCount(0, { timeout: 15_000 });
    await expect(summary(page)).toHaveText('Scoring (changed)');
    await openScoring(page);
    await page.getByRole('button', { name: 'Reset to default' }).click();
    await expect(flagged(page)).toHaveCount(2, { timeout: 15_000 });
    await expect(summary(page)).toHaveText('Scoring');

    // Settings shows what the toolbar saved, and resets to the built-in values.
    const again = await openScoringDefaults(page);
    await expect(again.getByRole('spinbutton', { name: 'Min stars' })).toHaveValue('515');
    await again.getByRole('button', { name: 'Reset to built-in defaults' }).click();
    await expect(again.getByRole('spinbutton', { name: 'Max HFR (pixels)' })).toHaveValue('');
    await closeSettings(page);
    await expect(flagged(page)).toHaveCount(0, { timeout: 15_000 });
  });

  test('a view\'s scoring follows it within the project, says so, and stays behind', async ({ page }) => {
    await openSequence(page, `/#/sequence?db=${encodeURIComponent(dbId)}&project=1&target=1`);
    await openScoring(page);
    const minStars = page.getByLabel('Min stars');
    await minStars.fill('505');
    await minStars.press('Enter');
    await expect(flagged(page)).toHaveCount(1, { timeout: 15_000 });
    await expect(page).toHaveURL(/minStars=505/);

    // Images keeps it, and says so beside its counts.
    await page.getByRole('button', { name: 'Images', exact: true }).click();
    await expect(page.locator('.image-card')).toHaveCount(3, { timeout: 15_000 });
    await expect(page).toHaveURL(/minStars=505/);
    await expect(page.locator('.view-toolbar-summary .scoring-changed-note'))
      .toContainText('Scoring (changed)');

    // So does the detail view, above the frame's quality.
    await page.locator('.image-card').first().dblclick();
    await expect(page).toHaveURL(/#\/detail\/\d+\?.*minStars=505/);
    await expect(page.locator('.detail-info .scoring-changed-note')).toContainText('Scoring (changed)');

    // Back to Images, still with it, then Stacks, which says in full
    // beside Build what a build would use.
    await page.keyboard.press('Escape');
    await expect(page).toHaveURL(/#\/grid\?.*minStars=505/);
    await page.getByRole('button', { name: 'Stacks', exact: true }).click();
    const build = page.locator('.scoring-changed-build');
    await expect(build).toContainText('Builds here use this view\'s scoring', { timeout: 15_000 });
    await expect(build).toContainText('Min stars 505 (default off)');

    // Another project starts from the defaults.
    await page.locator('#scope-select').click();
    const picker = page.getByRole('dialog', { name: 'Choose a project or target' });
    await page.getByLabel('Search projects or targets').fill('Beta Field');
    await picker.getByRole('button', { name: /^Beta Field/ }).click();
    await expect(page).toHaveURL(/project=2/);
    await expect(page).not.toHaveURL(/minStars/);
    await expect(page.locator('.scoring-changed-note')).toHaveCount(0);
  });

  test('a value Settings cannot hold is refused', async ({ page }) => {
    await openSequence(page, `/#/sequence?db=${encodeURIComponent(dbId)}&project=1&target=1`);
    const defaults = await openScoringDefaults(page);
    const pointing = defaults.getByRole('spinbutton', { name: 'Pointing (percent)' });
    await pointing.fill('250');
    await expect(pointing).toHaveAttribute('aria-invalid', 'true');
    await expect(defaults.getByRole('alert')).toHaveText('Enter 0 to 200%, in steps of 10.');
    await expect(defaults.getByRole('button', { name: 'Save defaults' })).toBeDisabled();
  });
});

test.describe('with accounts', () => {
  let accounts: AccountsServer;

  test.describe.configure({ mode: 'serial' });

  test.beforeAll(async () => {
    accounts = await startAccountsServer('scoring-defaults', 8);
  });

  test.afterAll(() => {
    accounts?.stop();
  });

  const sequence = () =>
    `${accounts.base}/#/sequence?db=${encodeURIComponent(accounts.dbId())}&project=1&target=1`;

  test('an editor\'s earlier defaults reach the server once, and every user scores with them', async ({ browser, playwright }) => {
    // Before the server kept a copy, bob, an editor, saved his own with his
    // account. Seed them before any page of his loads; the server has none.
    const api = await playwright.request.newContext({ baseURL: accounts.base });
    expect((await api.post('/api/auth/login', { data: { username: 'bob', password: ACCOUNT_PASSWORD } })).ok())
      .toBeTruthy();
    expect((await api.patch('/api/me/preferences', {
      data: { schema_version: 1, preferences: { scoringHfrRejectAbove: 2.45 } },
    })).ok()).toBeTruthy();
    expect((await (await api.get('/api/settings/scoring')).json()).data.stored).toBe(false);
    await api.dispose();

    // His first page after the update copies them to the server.
    const bob = await accounts.signIn(browser, 'bob');
    await expect.poll(async () =>
      (await (await bob.context().request.get(`${accounts.base}/api/settings/scoring`)).json()).data.scoring
        .hfr_reject_above, { timeout: 15_000 }).toBe(2.45);
    await openSequence(bob, sequence());
    await expect(flagged(bob)).toHaveCount(2, { timeout: 15_000 });
    await expect(summary(bob)).toHaveText('Scoring');
    await bob.context().close();

    // Alice, a viewer, scores with the same defaults in her own browser,
    // and cannot save others.
    const alice = await accounts.signIn(browser, 'alice');
    await openSequence(alice, sequence());
    await expect(flagged(alice)).toHaveCount(2, { timeout: 15_000 });
    await openScoring(alice);
    await expect(alice.getByLabel('Max HFR')).toHaveValue('2.45');
    // Settings is for editors, so the popover offers no way there.
    await expect(alice.getByRole('button', { name: 'Edit defaults' })).toHaveCount(0);
    const minStars = alice.getByLabel('Min stars');
    await minStars.fill('515');
    await minStars.press('Enter');
    await expect(summary(alice)).toHaveText('Scoring (changed)');
    await expect(alice.getByRole('button', { name: 'Save as default' })).toBeDisabled();
    await alice.screenshot({ path: test.info().outputPath('scoring-control-viewer.png') });
    await alice.context().close();
  });

  test('a change an editor saves reaches every user, and is copied no more', async ({ browser }) => {
    // Bob saves new defaults in Settings; the server's copy changes.
    const bob = await accounts.signIn(browser, 'bob');
    await openSequence(bob, sequence());
    const saved = bob.waitForResponse((response) =>
      response.url().endsWith('/api/settings/scoring')
      && response.request().method() === 'PUT'
      && response.ok()
      && (response.request().postData() ?? '').includes('"star_count_reject_below":505'));
    const defaults = await openScoringDefaults(bob);
    await defaults.getByRole('spinbutton', { name: 'Max HFR (pixels)' }).fill('');
    await defaults.getByRole('spinbutton', { name: 'Min stars' }).fill('505');
    await defaults.getByRole('button', { name: 'Save defaults' }).click();
    await saved;
    await closeSettings(bob);
    await expect(flagged(bob)).toHaveCount(1, { timeout: 15_000 });
    await bob.context().close();

    // Alice, in a fresh browser, sees bob's new defaults.
    const alice = await accounts.signIn(browser, 'alice');
    await openSequence(alice, sequence());
    await expect(flagged(alice)).toHaveCount(1, { timeout: 15_000 });
    await openScoring(alice);
    await expect(alice.getByLabel('Min stars')).toHaveValue('505');
    await expect(alice.getByLabel('Max HFR')).toHaveValue('');
    await alice.context().close();

    // Bob's account still holds his older values, but the server has a
    // copy now, so a new page does not copy them over it.
    const bobAgain = await accounts.signIn(browser, 'bob');
    let copiedAgain = false;
    bobAgain.on('request', (request) => {
      if (request.url().endsWith('/api/settings/scoring') && request.method() === 'PUT') copiedAgain = true;
    });
    await openSequence(bobAgain, sequence());
    await expect(flagged(bobAgain)).toHaveCount(1, { timeout: 15_000 });
    expect(copiedAgain).toBe(false);
    await bobAgain.context().close();
  });
});
