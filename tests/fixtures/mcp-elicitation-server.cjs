// Local manual-check MCP server. Test values only; no filesystem reads or outbound requests.
const readline = require('node:readline');
const http = require('node:http');
const pending = new Map();
let nextId = 0;
const send = value => process.stdout.write(`${JSON.stringify(value)}\n`);
const reply = (id, result) => send({jsonrpc:'2.0',id,result});
function elicit(message, requestedSchema) {
  const id = `manual-${++nextId}`;
  const answer = new Promise(resolve => pending.set(id,resolve));
  send({jsonrpc:'2.0',id,method:'elicitation/create',params:{mode:'form',message,requestedSchema}});
  return answer;
}
let browserServer;
let browserPort;
const liveUrls = new Set();
async function elicitUrl() {
  if (!browserServer) {
    browserServer = http.createServer((request, response) => {
      const url = new URL(request.url, 'http://127.0.0.1');
      const id = url.searchParams.get('id');
      response.setHeader('Content-Type','text/html; charset=utf-8');
      if (url.pathname === '/complete' && liveUrls.delete(id)) {
        send({jsonrpc:'2.0',method:'notifications/elicitation/complete',params:{elicitationId:id}});
        response.end('<h1>MCP URL check complete</h1><p>Return to the terminal.</p>');
      } else if (liveUrls.has(id)) {
        response.end(`<h1>MCP URL check</h1><p>Click while the terminal URL request is still open.</p><a href="/complete?id=${encodeURIComponent(id)}">Complete this request</a>`);
      } else { response.statusCode=404; response.end('No pending URL check.'); }
    });
    await new Promise(resolve => browserServer.listen(0,'127.0.0.1',resolve));
    browserPort = browserServer.address().port;
  }
  const id = `manual-url-${++nextId}`;
  liveUrls.add(id);
  const answer = new Promise(resolve => pending.set(id,resolve));
  send({jsonrpc:'2.0',id,method:'elicitation/create',params:{mode:'url',message:'MCP-URL-B: this browser must wait for A.',elicitationId:id,url:`http://127.0.0.1:${browserPort}/?id=${id}`}});
  return await answer;
}
const basicSchema = {type:'object',properties:{
  display_name:{type:'string',title:'Display name',minLength:2,maxLength:20},
  retry_count:{type:'integer',title:'Retry count',minimum:0,maximum:3},
  enabled:{type:'boolean',title:'Enabled'},
  color:{type:'string',title:'Color',oneOf:[{const:'red',title:'Red label'},{const:'blue',title:'Blue label'}]},
  labels:{type:'array',title:'Labels',minItems:1,maxItems:2,items:{type:'string',enum:['docs','rust','sdk']}},
  optional_note:{type:'string',title:'Optional note'},
},required:['display_name','retry_count','enabled','color','labels']};
const textSchema = title => ({type:'object',properties:{answer:{type:'string',title,minLength:2}},required:['answer']});
async function call(name) {
  switch(name) {
    case 'form_basics': return await elicit('MCP-FORM-BASICS: enter test values only.',basicSchema);
    case 'form_queue': return await Promise.all([
      elicit('MCP-FORM-A: keep editing when B queues.',textSchema('Answer A')),
      elicit('MCP-FORM-B: this must wait for A.',textSchema('Answer B')),
    ]);
    case 'form_url_queue': return await Promise.all([elicit('MCP-URL-A: no browser should open yet.',textSchema('Answer before URL')),elicitUrl()]);
    case 'form_formats': return await elicit('MCP-FORM-FORMATS: test validation.',{type:'object',properties:{
      date:{type:'string',format:'date'}, email:{type:'string',format:'email'},
      uri:{type:'string',format:'uri'}, timestamp:{type:'string',format:'date-time'},
      fraction:{type:'number',minimum:0,maximum:1,default:0.5}
    },required:['date','email','uri','timestamp','fraction']});
    default: throw new Error('Unknown fixture tool');
  }
}
const input = readline.createInterface({input:process.stdin});
input.on('close', () => { browserServer?.close(); });
input.on('line', async line => {
  let message;
  try { message = JSON.parse(line); } catch { return; }
  if (message.id !== undefined && !message.method) {
    const resolve = pending.get(message.id);
    pending.delete(message.id);
    resolve?.(message.result ?? {error:true});
    return;
  }
  if (message.method === 'initialize') reply(message.id,{protocolVersion:message.params.protocolVersion,capabilities:{tools:{}},serverInfo:{name:'elicitation-manual',version:'1.0.0'}});
  else if (message.method === 'ping') reply(message.id,{});
  else if (message.method === 'tools/list') reply(message.id,{tools:['form_basics','form_queue','form_formats','form_url_queue'].map(name=>({name,description:`Manual MCP elicitation check: ${name}. Test values only.`,inputSchema:{type:'object',properties:{}}}))});
  else if (message.method === 'tools/call') {
    try {
      const result = await call(message.params.name);
      reply(message.id,{content:[{type:'text',text:JSON.stringify(result)}]});
    } catch { reply(message.id,{isError:true,content:[{type:'text',text:'Fixture tool failed.'}]}); }
  }
});
