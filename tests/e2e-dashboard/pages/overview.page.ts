import { type Page, type Locator } from '@playwright/test';

export class OverviewPage {
  readonly page: Page;
  readonly heading: Locator;
  readonly statusCard: Locator;
  readonly statusDiv: Locator;
  readonly issuesCard: Locator;
  readonly issuesList: Locator;
  readonly tabs: Locator;
  // New overview elements (issue #948)
  readonly agentLiveStatus: Locator;
  readonly agentLiveStatusCard: Locator;
  readonly recentActionsList: Locator;
  readonly recentActionsCard: Locator;
  readonly openPrsList: Locator;
  readonly openPrsCard: Locator;
  readonly clusterTopology: Locator;
  readonly clusterTopologyCard: Locator;
  readonly remoteVms: Locator;
  readonly remoteVmsCard: Locator;
  readonly hostsList: Locator;
  readonly hostsCard: Locator;
  readonly hostNameInput: Locator;
  readonly hostRgInput: Locator;

  constructor(page: Page) {
    this.page = page;
    this.heading = page.locator('text=Simard Dashboard');
    this.statusCard = page.locator('#tab-overview .card:has(#status)');
    this.statusDiv = page.locator('#status');
    this.issuesCard = page.locator('#tab-overview .card:has(#issues-list)');
    this.issuesList = page.locator('#issues-list');
    this.tabs = page.locator('.tab');
    // New overview elements
    this.agentLiveStatus = page.locator('#agent-live-status');
    this.agentLiveStatusCard = page.locator('#tab-overview .card:has(#agent-live-status)');
    this.recentActionsList = page.locator('#recent-actions-list');
    this.recentActionsCard = page.locator('#tab-overview .card:has(#recent-actions-list)');
    this.openPrsList = page.locator('#open-prs-list');
    this.openPrsCard = page.locator('#tab-overview .card:has(#open-prs-list)');
    this.clusterTopology = page.locator('#cluster-topology');
    this.clusterTopologyCard = page.locator('#tab-overview .card:has(#cluster-topology)');
    this.remoteVms = page.locator('#remote-vms');
    this.remoteVmsCard = page.locator('#tab-overview .card:has(#remote-vms)');
    this.hostsList = page.locator('#hosts-list');
    this.hostsCard = page.locator('#tab-overview .card:has(#hosts-list)');
    this.hostNameInput = page.locator('#host-name');
    this.hostRgInput = page.locator('#host-rg');
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
