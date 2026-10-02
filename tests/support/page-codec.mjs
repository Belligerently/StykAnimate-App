/** In a page of the app (the development server serves /tests): one request to the Pivot codec,
 * made as an import or export job makes it, through the page's codec worker. Answers like the
 * HTTP codec service did, with a standard Response.
 *
 *   const {codecFetch} = await import('/tests/support/page-codec.mjs');
 *   const response = await codecFetch('/api/export', {method: 'POST', body: JSON.stringify({document, format: 'piv'})});
 */
import {codecJob} from '/src/codec-job.mjs';
import {codecRequest} from '/src/codec-request.mjs';

export async function codecFetch(path, init = {}) {
  const job = codecJob();
  try { return await codecRequest(job.link().fields, String(path), init); }
  finally { job.finish(); }
}
