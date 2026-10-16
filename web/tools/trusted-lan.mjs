// Explicit HTTP/WS LAN gateway; the Rust backend still verifies its private CA.
import {createServer} from 'node:http';
import {connect} from 'node:tls';
import {readFile,realpath,writeFile} from 'node:fs/promises';
import {resolve,extname} from 'node:path';
import {homedir,hostname,networkInterfaces} from 'node:os';
import {pathToFileURL} from 'node:url';
export function trustedAddress(address='') {
 const parts=address.replace(/^::ffff:/,'').split('.').map(Number);
 return parts.length===4 && parts.every(n=>Number.isInteger(n)&&n>=0&&n<=255) && (parts[0]===127 || parts[0]===10 || parts[0]===192&&parts[1]===168 || parts[0]===172&&parts[1]>=16&&parts[1]<=31 || parts[0]===169&&parts[1]===254);
}
export function allowedHosts(port, interfaces=networkInterfaces(), name=hostname()) {
 const names=new Set(['localhost','127.0.0.1',name.toLowerCase()]);
 for(const addresses of Object.values(interfaces))for(const address of addresses??[])if(address.family==='IPv4'&&trustedAddress(address.address))names.add(address.address);
 return new Set(Array.from(names,name=>`${name}:${port}`));
}
export function approvedRequest(req,hosts,websocket=false) {
 if(!trustedAddress(req.socket?.remoteAddress))return false;
 const host=req.headers.host;
 if(!host||!hosts.has(host.toLowerCase()))return false;
 const origin=req.headers.origin;
 if(origin&&origin!==`http://${host.toLowerCase()}`)return false;
 return !websocket||(origin===`http://${host.toLowerCase()}`&&req.method==='GET'&&req.url==='/control'&&req.headers.upgrade?.toLowerCase()==='websocket'&&req.headers['sec-websocket-protocol']==='soundroom.v3'&&req.headers['sec-websocket-version']==='13'&&typeof req.headers['sec-websocket-key']==='string'&&/^[A-Za-z0-9+/]{22}==$/.test(req.headers['sec-websocket-key']));
}
export function upgradeRequest(headers,serverPort) {
 return `GET /control HTTP/1.1\r\nHost: localhost:${serverPort}\r\nOrigin: https://localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: soundroom.v3\r\nSec-WebSocket-Key: ${headers['sec-websocket-key']}\r\n\r\n`;
}
export async function startTrustedLAN({port=5174,serverPort=49160,caPath=resolve(homedir(),'Library/Application Support/Soundroom/RustServer/tls/ca.crt'),bind='0.0.0.0'}={}) {
 const reportPath=new URL('../../build/QA/trusted-lan-runtime-20261012.json',import.meta.url);
 const events=[];let writing=false,dirty=false;
 const report=(stage,code)=>{
  events.push({at:new Date().toISOString(),stage,...(code?{code}:{})});if(events.length>32)events.shift();dirty=true;
  if(writing)return;writing=true;
  void(async()=>{try{while(dirty){dirty=false;await writeFile(reportPath,JSON.stringify({events})+'\n',{mode:0o600});}}catch{}finally{writing=false;}})();
 };
 const peers=new Set();
 const ca=await readFile(caPath),root=await realpath(new URL('../dist',import.meta.url)),hosts=allowedHosts(port);
 const server=createServer(async(req,res)=>{
  if(!approvedRequest(req,hosts)||!['GET','HEAD'].includes(req.method)){res.writeHead(403);res.end('Unapproved host or origin');return;}
  const headers={'Cache-Control':'no-store','X-Content-Type-Options':'nosniff','Referrer-Policy':'no-referrer','Content-Security-Policy':`default-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self' ws://${req.headers.host}; frame-ancestors 'none'; object-src 'none'; base-uri 'none'`};
  try {
   const raw=(req.url??'/').split('?')[0];
   if(raw==='/soundroom-config.json'){res.writeHead(200,{...headers,'Content-Type':'application/json'});res.end(JSON.stringify({wssURL:`ws://${req.headers.host}/control`,trustedLAN:true}));return;}
   const path=resolve(root,'.'+decodeURIComponent(raw==='/'?'/index.html':raw));
   if(!path.startsWith(root+'/'))throw Error();
   const canonical=await realpath(path);if(!canonical.startsWith(root+'/'))throw Error();
   const body=await readFile(canonical);res.writeHead(200,{...headers,'Content-Type':({'.html':'text/html','.js':'text/javascript','.css':'text/css','.json':'application/json'})[extname(path)]??'application/octet-stream'});res.end(req.method==='HEAD'?undefined:body);
  }catch{res.writeHead(404,headers);res.end('Not found');}
 });
 server.on('upgrade',(req,socket,head)=>{
  if(!approvedRequest(req,hosts,true)){report('browser-upgrade-rejected');socket.end('HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n');return;}
  report('connecting-backend');
  if(peers.size>=16){socket.end('HTTP/1.1 503 Service Unavailable\r\nConnection: close\r\n\r\n');return;}
  peers.add(socket);socket.once('close',()=>peers.delete(socket));
  const upstream=connect({host:'127.0.0.1',port:serverPort,servername:'localhost',ca,rejectUnauthorized:true});
  const fail=()=>{socket.destroy();upstream.destroy();};socket.on('error',fail);upstream.on('error',error=>{report('backend-connection-failed',error.code??'TLS/transport error');console.error('Trusted LAN backend connection failed:',error.code??'TLS/transport error');fail();});socket.on('close',()=>upstream.destroy());upstream.on('close',()=>socket.destroy());
  let upgraded=false;
  upstream.once('data',bytes=>{upgraded=bytes.toString('ascii',0,Math.min(bytes.length,64)).startsWith('HTTP/1.1 101 ');report(upgraded?'websocket-upgraded':'backend-upgrade-rejected');if(!upgraded)console.error('Trusted LAN: backend did not accept the WebSocket upgrade.');});
  upstream.once('close',()=>{if(!upgraded){report('backend-closed-before-upgrade');console.error('Trusted LAN: backend closed before WebSocket upgrade.');}});
  const timer=setTimeout(fail,10000);upstream.once('secureConnect',()=>{
   clearTimeout(timer);report('backend-tls-verified');
   upstream.write(upgradeRequest(req.headers,serverPort));if(head.length)upstream.write(head);upstream.pipe(socket);socket.pipe(upstream);
  });upstream.once('close',()=>clearTimeout(timer));
 });
 await new Promise((resolve,reject)=>{server.once('error',reject);server.listen(port,bind,resolve);});return {server,hosts};
}
if(process.argv[1]&&import.meta.url===pathToFileURL(resolve(process.argv[1])).href){
 const port=Number(process.env.SOUNDROOM_LAN_PORT??5174),serverPort=Number(process.env.SOUNDROOM_SERVER_PORT??49160);
 if(![port,serverPort].every(p=>Number.isInteger(p)&&p>0&&p<=65535))throw Error('Ports must be 1–65535.');
 const {hosts}=await startTrustedLAN({port,serverPort,...(process.env.SOUNDROOM_CA?{caPath:process.env.SOUNDROOM_CA}:{})});
 console.log('Trusted LAN: unencrypted HTTP/WebSocket, pairing required. Keep Soundroom-Rust.app running.');for(const host of hosts)console.log(`http://${host}/`);
}
