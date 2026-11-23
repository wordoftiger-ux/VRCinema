// Moves the request page to a view, whatever the width: the bottom tabs on a phone, the
// two columns and the drawer on a wide screen (shared by the viewer checks).

export async function showAdd(page) {
  await page.waitForTimeout(120); // a request just sent may be about to move the page to the queue
  if (await page.locator('.v-add').count()) return;
  if (await page.locator('.v-tabs').count()) await page.click('.v-tab[data-tab=add]');
  else await page.click('[data-action=add-film]');
  await page.waitForSelector('.v-add');
}

export async function showQueue(page) {
  if (await page.locator('.v-tabs').count()) await page.click('.v-tab[data-tab=queue]');
  await page.waitForSelector('.v-queue');
}

export async function showNow(page) {
  if (await page.locator('.v-tabs').count()) await page.click('.v-tab[data-tab=now]');
  await page.waitForSelector('.v-now');
}
