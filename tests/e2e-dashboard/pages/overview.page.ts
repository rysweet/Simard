import { type Page, type Locator } from '@playwright/test';

export class OverviewPage {
  readonly page: Page;
  readonly heading: Locator;
  readonly statusCard: Locator;
  readonly statusDiv: Locator;
  readonly issuesCard: Locator;
  readonly issuesList: Locator;
  readonly tabs: Locator;
  readonly agentLiveStatusCard: Locator;
  readonly agentLiveStatus: Locator;
  readonly recentActionsCard: Locator;
  readonly recentActionsList: Locator;
  readonly openPrsCard: Locator;
  readonly openPrsList: Locator;
  readonly mergeReadinessCard: Locator;

  constructor(page: Page) {
    this.page = page;
    this.heading = page.locator('text=Simard Dashboard');
    this.statusCard = page.locator('#tab-overview .card:has(#status)');
    this.statusDiv = page.locator('#status');
    this.issuesCard = page.locator('#tab-overview .card:has(#issues-list)');
    this.issuesList = page.locator('#issues-list');
    this.tabs = page.locator('.tab');
    this.agentLiveStatusCard = page.locator('#tab-overview .card:has(#agent-live-status)');
    this.agentLiveStatus = page.locator('#agent-live-status');
    this.recentActionsCard = page.locator('#tab-overview .card:has(#recent-actions-list)');
    this.recentActionsList = page.locator('#recent-actions-list');
    // Retained to assert the duplicative Open PRs card is GONE (#26).
    this.openPrsCard = page.locator('#tab-overview .card:has(#open-prs-list)');
    this.openPrsList = page.locator('#open-prs-list');
    // Merge Readiness is the single Overview PR surface kept after the removal.
    this.mergeReadinessCard = page.locator(
      '#tab-overview .card[data-testid="merge-readiness-card"]',
    );
  }

  async getTabNames(): Promise<string[]> {
    const count = await this.tabs.count();
    const names: string[] = [];
    for (let i = 0; i < count; i++) {
      names.push((await this.tabs.nth(i).textContent()) ?? '');
    }
    return names;
  }

  async clickTab(name: string): Promise<void> {
    await this.page.locator(`.tab[data-tab="${name}"]`).click();
  }

  async isTabContentVisible(name: string): Promise<boolean> {
    return this.page.locator(`#tab-${name}`).isVisible();
  }
}
