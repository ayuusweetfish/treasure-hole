import { serve } from 'https://deno.land/std@0.117.0/http/server.ts';
import { strftime } from './strftime.js';
import staticFiles from "https://deno.land/x/static_files@1.1.4/mod.ts";

const addr = ':1213';

const EXEC_PATH = '../target/release/treasurehole';
// Use proxy if needed, e.g.
// const PROXY = 'http://127.0.0.1:1087';
const PROXY = '';

const indexHtmlContents = await Deno.readFile('index.html');

const downloads = staticFiles('./data', {
  prefix: '/download'
});

// id => { log };
const ctx = {};

const handler = async (req) => {
  const method = req.method;
  const path = new URL(req.url).pathname;

  if (method === 'GET' && path === '/') {
    return new Response(indexHtmlContents, { status: 200 });
  }

  const requestMatch = path.match(/^\/request\/([a-z0-9]{1,50})\/(\d\d?)$/);
  if (method === 'POST' && requestMatch !== null) {
    const token = requestMatch[1];
    const levels = parseInt(requestMatch[2], 10);
    if (levels < 0 || levels > 10) {
      return new Response(null, { status: 400 });
    }
    const date = strftime('%Y%m%d-%H%M%S');
    const id = `${date}-${token}`;
    if (ctx[id] !== undefined) {
      return new Response(null, { status: 400 });
    }
    const log = [];
    ctx[id] = {
      log,
    };
    const worker = async () => {
      const cmd = [
        EXEC_PATH,
        token,
        levels.toString(),
        `./data/${id}`,
        PROXY,
      ];
      const proc = Deno.run({
        cmd,
        stdout: 'null',
        stderr: 'piped',
      });
      console.log(proc.stderr);
      const buf = new Uint8Array(4096);
      const decoder = new TextDecoder();
      const line = new Uint8Array(4096);
      let linePtr = 0;
      while (true) {
        const bytesRead = await proc.stderr.read(buf);
        if (bytesRead === null) break;
        for (let i = 0; i < bytesRead; i++) {
          if (buf[i] === 10) {
            log.splice(0, 0, decoder.decode(line.slice(0, linePtr)));
            if (log.length >= 100) log.pop();
            linePtr = 0;
          } else if (linePtr < line.length) {
            line[linePtr++] = buf[i];
          }
        }
      }
      if (linePtr !== 0) {
        log.splice(0, 0, decoder.decode(line.slice(0, linePtr)));
        if (log.length >= 100) log.pop();
      }
      // Successful?
      if (log[0] === '完成') {
        log.splice(0, 0, '压缩中\n======');
        // Compress
        const proc = Deno.run({
          cwd: './data',
          cmd: ['zip', id, '-r', id],
          stdout: 'null',
          stderr: 'null',
        });
        const { code } = await proc.status();
        if (code !== 0) {
          log.splice(0, 0, `压缩过程出现意外问题：${code}`);
        } else {
          log.splice(0, 0, '完成');
        }
      }
      ctx[id].fin = true;
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

  const zipMatch = path.match(/^\/download\/([a-z0-9\-]{1,50}).zip$/);
  if (method === 'GET' && zipMatch !== null) {
    const id = zipMatch[1];
    return downloads({
      request: req,
      respondWith: r => r,
    });
  }

  return new Response('Not found', { status: 404 });
};

console.log(`Running at http://localhost${addr}/`);
await serve(handler, { addr });
