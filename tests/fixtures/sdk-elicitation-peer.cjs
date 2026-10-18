// Fake CLI control peer for a real pinned-SDK transport characterization. No network or model.
const readline = require('node:readline');
const send = value => process.stdout.write(`${JSON.stringify(value)}\n`);
process.on('message', message => {
  if (message.cancel) send({ type: 'control_cancel_request', request_id: message.cancel });
});
readline.createInterface({input:process.stdin}).on('line', line => {
  const message = JSON.parse(line);
  if (message.type === 'control_request') {
    send({type:'control_response', response:{subtype:'success',request_id:message.request_id,response:{commands:[],models:[],account:{}}}});
    if (message.request.subtype === 'initialize') {
      for (const id of ['sdk-form-a','sdk-form-b']) send({type:'control_request',request_id:id,request:{subtype:'elicitation',
        mcp_server_name:'characterization',message:id,mode:'form',
        requested_schema:{type:'object',properties:{zulu:{type:'boolean'},alpha:{type:'string'}}},
        property_order:['alpha','zulu']}});
    }
  } else if (message.type === 'control_response') {
    process.send?.(message);
  }
});
