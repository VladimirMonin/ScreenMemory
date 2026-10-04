import type { SearchReply } from '../../../packages/protocol/src/generated/application-v1';
import type { SearchRequest } from '../../../packages/protocol/src/generated/application-methods-v1';

type Job = { generation: number; request: SearchRequest; cancelled: boolean; issued: boolean; cancelSent: boolean; searchId: string | null };
/** One in-flight request + ONE replaceable latest pending. No busy polling/retry. */
export class LatestSearchLane {
  private active: Job | null = null;
  private pending: Job | null = null;
  private window: string | null = null;
  private disposed = false;
  constructor(private send: (request: SearchRequest) => Promise<SearchReply>,
    private complete: (generation: number, reply: SearchReply | null, error: unknown) => void) {}
  enqueue(generation: number, request: SearchRequest) {
    if (this.disposed) return;
    this.pending = { generation, request, cancelled: false, issued: false, cancelSent: false,
      searchId: request.action === 'start' ? request.search_id : this.window };
    if (!this.active) void this.drain();
  }
  invalidate() {
    this.pending = null;
    if (this.active) { this.active.cancelled = true; this.cancel(this.active); }
  }
  private cancel(job: Job) {
    if (!job.issued || job.cancelSent || !job.searchId) return;
    job.cancelSent = true;
    // Native cancel signals this window's token without its session mutex. The
    // active promise still drains its terminal response before latest admission.
    void this.send({ action: 'cancel', search_id: job.searchId }).catch(error => {
      if (!this.disposed && !job.cancelled) this.complete(job.generation, null, error);
    });
  }
  dispose() { this.disposed = true; this.invalidate(); if (!this.active) this.releaseWindow(); }
  private releaseWindow() {
    const id = this.window; this.window = null;
    if (id) void this.send({ action: 'cancel', search_id: id }).catch(() => undefined);
  }
  private async drain() {
    while (!this.disposed && this.pending) {
      const job = this.pending; this.pending = null; this.active = job;
      let reply: SearchReply | null = null; let error: unknown = null;
      try {
        if (job.request.action === 'start' && this.window) {
          const id = this.window; this.window = null;
          await this.send({ action: 'cancel', search_id: id }).catch(reason => {
            // An expired previous cache entry is already released. Busy/transport/
            // permission errors are NOT hidden or retried as successful admission.
            if (reason !== 'SEARCH_EXPIRED') throw reason;
          });
        }
        if (!job.cancelled && !this.disposed) {
          job.issued = true;
          reply = await this.send(job.request);
          if (reply.kind === 'page') this.window = reply.value.search_id;
        }
      } catch (reason) { error = reason; }
      finally {
        this.active = null;
        if (!this.disposed && !job.cancelled) this.complete(job.generation, reply, error);
      }
    }
    if (this.disposed) this.releaseWindow();
  }
}
