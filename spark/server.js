// deno run --allow-net --allow-read --allow-env %

import { serve } from 'https://deno.land/std@0.117.0/http/server.ts';
import { strftime } from '../server/strftime.js';

// Do not go gentle into that good night

const addr = ':1937';

const indexHtmlContents = await Deno.readFile('index.html');

// id => { log, fin, list };
const ctx = {};

Deno.env.set('TZ', 'Asia/Shanghai');
const timestr = (ts) => strftime('%F %H:%M:%S', new Date(ts * 1000));

const handler = async (req) => {
  const method = req.method;
  const path = new URL(req.url).pathname;

  if (method === 'GET' && path === '/') {
    return new Response(indexHtmlContents, { status: 200 });
  }

  const requestMatch = path.match(/^\/request\/([a-z0-9]{1,50})\/([01])$/);
  if (method === 'POST' && requestMatch !== null) {
    const token = requestMatch[1];
    const cancelAttention = (requestMatch[2] === '0' ? false : true);
    const date = strftime('%Y%m%d-%H%M%S');
    const id = `${date}-${token}`;
    if (ctx[id] !== undefined) {
      return new Response(null, { status: 400 });
    }
    const log = [];
    const list = [];
    ctx[id] = {
      log,
      list,
    };
    const worker = async () => {
      console.log(`${strftime('%F %H:%M:%S')} Starting ${id}`);
      let count = 0;
      for (let page = 1; ; page++) {
        let requestPage = (cancelAttention ? 1 : page);
        const obj = await (await fetch(
          `https://tapi.thuhole.com/v3/contents/post/attentions?page=${requestPage}`, {
            headers: { 'TOKEN': token },
          }
        )).json();
        if (obj.code !== 0) {
          log.splice(0, 0, `树洞返回错误：\n${obj.msg}\n======`);
          break;
        }
        if (obj.count === 0) break;
        for (const post of obj.data) {
          list.push(
`=== #${post.pid} === ${timestr(post.timestamp)}
${post.text}`
          );
          const cmts = obj.comments[post.pid];
          if (cmts) {
            for (const cmt of cmts)
              list.push(
`[${cmt.name}] ${timestr(cmt.timestamp)}
${cmt.text}`
              );
            if (post.reply > cmts.length) {
              list.push(
`(还有 ${post.reply - cmts.length} 条)`
              );
            }
          }
          // Cancel attention?
          if (cancelAttention) {
            const body = new FormData();
            body.append('pid', post.pid.toString());
            body.append('switch', '0');
            const obj = await (await fetch(
              `https://tapi.thuhole.com/v3/edit/attention`, {
                method: 'POST',
                headers: { 'TOKEN': token },
                body,
              }
            )).json();
          }
        }
        count += obj.data.length;
        const word = (Math.random() <= 0.01 ? '咕咕。' : '咕嘟。');
        log.splice(0, 0, `${word}第 ${page} 页，合计 ${count} 条`);
        if (log.length >= 50) log.pop();
      }
      log.splice(0, 0, `完成`);
      log.splice(1, 0, `有效期 10 分钟，请及时下载~\n======`);
      ctx[id].fin = true;
      console.log(`${strftime('%F %H:%M:%S')} Finished ${id}`);
      // Remove after 10 minutes
      setTimeout(() => delete ctx[id], 600000);
    };
    worker(); // Do not await
    return new Response(id, { status: 200 });
  }

  const logMatch = path.match(/^\/log\/([a-z0-9\-]{1,50})$/);
  if (method === 'GET' && logMatch !== null) {
    const id = logMatch[1];
    const c = ctx[id];
    if (c === undefined) {
      return new Response('Not found', { status: 404 });
    }
    return new Response(JSON.stringify({
      fin: !!c.fin,
      log: c.log,
    }), { status: 200 });
  }

  const downloadMatch = path.match(/^\/download\/([a-z0-9\-]{1,50})$/);
  if (method === 'GET' && downloadMatch !== null) {
    const id = downloadMatch[1];
    const c = ctx[id];
    if (c === undefined) {
      return new Response('Not found', { status: 404 });
    }
    return new Response(c.list.join('\n\n'), {
      headers: {
        'Content-Type': 'text/plain',
        'Content-Disposition': 'attachment; filename="attentions.txt"',
      },
      status: 200,
    });
  }

  return new Response('Not found', { status: 404 });
};

console.log(`Running at http://localhost${addr}/`);
await serve(handler, { addr });
