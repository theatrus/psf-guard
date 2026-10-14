import { expect, test, type Page } from '@playwright/test';
import { startAccountsServer, type AccountsServer } from './accounts-server';
import {
  openSettings,
  registerFixtureDb,
  resetDatabases,
  waitForCacheReady,
} from './helpers';

// Sequence's scoring defaults: set in Settings, applied by every view, and
// changed per view in the toolbar until saved. The fixture's Alpha sequence
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
  await page.getByRole('tab', { name: 'Review' }).click();
  return page.getByRole('region', { name: 'Sequence scoring' });
}

async function closeSettings(page: Page) {
  await page.locator('.tauri-settings .close-button').click();
  await expect(page.getByRole('heading', { name: 'PSF Guard Settings' })).toHaveCount(0);
}

test.describe('without accounts', () => {
  let dbId: string;

  test.beforeEach(async ({ request }) => {
    await resetDatabases(request);
    const entry = await registerFixtureDb(request, {
      name: 'Imaging Rig e2e',
      slug: 'imaging-rig-e2e',
    });
    dbId = entry.id;
    await waitForCacheReady(request, dbId);
  });

  test('defaults set in Settings apply in Sequence, and the toolbar saves its own', async ({ page }) => {
    const sequenceUrl = `/#/sequence?db=${encodeURIComponent(dbId)}&project=1&target=1`;
    await openSequence(page, sequenceUrl);
    await expect(flagged(page)).toHaveCount(0);

    // Settings › Review › Sequence scoring.
    const defaults = await openScoringDefaults(page);
    await expect(defaults.getByRole('spinbutton', { name: 'Satellite trails (percent)' }))
      .toHaveValue('100');
    await defaults.getByRole('spinbutton', { name: 'Max HFR (pixels)' }).fill('2.55');
    await defaults.getByRole('button', { name: 'Save defaults' }).click();
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

  test('a user\'s scoring defaults follow them to a fresh browser, and stay theirs', async ({ browser }) => {
    // Bob, an editor, sets his own in Settings.
    const bob = await accounts.signIn(browser, 'bob');
    await openSequence(bob, sequence());
    const bobSaved = bob.waitForResponse((response) =>
      response.url().endsWith('/api/me/preferences')
      && response.request().method() === 'PATCH'
      && response.ok()
      && (response.request().postData() ?? '').includes('"scoringHfrRejectAbove":2.45'));
    const defaults = await openScoringDefaults(bob);
    await defaults.getByRole('spinbutton', { name: 'Max HFR (pixels)' }).fill('2.45');
    await defaults.getByRole('button', { name: 'Save defaults' }).click();
    await bobSaved;
    await closeSettings(bob);
    await expect(flagged(bob)).toHaveCount(2, { timeout: 15_000 });
    await bob.context().close();

    // Alice, a viewer, saves hers from the toolbar.
    const alice = await accounts.signIn(browser, 'alice');
    await openSequence(alice, sequence());
    await expect(flagged(alice)).toHaveCount(0, { timeout: 15_000 });
    await openScoring(alice);
    // Settings is for editors, so the popover offers no way there.
    await expect(alice.getByRole('button', { name: 'Edit defaults' })).toHaveCount(0);
    const minStars = alice.getByLabel('Min stars');
    await minStars.fill('505');
    await minStars.press('Enter');
    await expect(flagged(alice)).toHaveCount(1, { timeout: 15_000 });
    await expect(summary(alice)).toHaveText('Scoring (changed)');
    const aliceSaved = alice.waitForResponse((response) =>
      response.url().endsWith('/api/me/preferences')
      && response.request().method() === 'PATCH'
      && response.ok()
      && (response.request().postData() ?? '').includes('"scoringStarCountRejectBelow":505'));
    await alice.getByRole('button', { name: 'Save as default' }).click();
    await aliceSaved;
    await expect(summary(alice)).toHaveText('Scoring');
    await alice.reload();
    await expect(alice.locator('.sequence-image-card')).toHaveCount(3, { timeout: 30_000 });
    await expect(flagged(alice)).toHaveCount(1, { timeout: 15_000 });
    await alice.context().close();

    // Another browser: alice finds her defaults.
    const aliceAgain = await accounts.signIn(browser, 'alice');
    await openSequence(aliceAgain, sequence());
    await expect(flagged(aliceAgain)).toHaveCount(1, { timeout: 15_000 });
    await expect(summary(aliceAgain)).toHaveText('Scoring');
    await openScoring(aliceAgain);
    await expect(aliceAgain.getByLabel('Min stars')).toHaveValue('505');
    await expect(aliceAgain.getByLabel('Max HFR')).toHaveValue('');
    await aliceAgain.context().close();

    // Bob keeps his.
    const bobAgain = await accounts.signIn(browser, 'bob');
    await openSequence(bobAgain, sequence());
    await expect(flagged(bobAgain)).toHaveCount(2, { timeout: 15_000 });
    await openScoring(bobAgain);
    await expect(bobAgain.getByLabel('Max HFR')).toHaveValue('2.45');
    await expect(bobAgain.getByLabel('Min stars')).toHaveValue('');
    await bobAgain.context().close();
  });
});
