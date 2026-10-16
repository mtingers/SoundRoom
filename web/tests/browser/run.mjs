import {resolve} from 'node:path';import {spawn} from 'node:child_process';
if(process.platform!=='darwin'){console.error('WebKit acceptance requires macOS; no substitute result is reported.');process.exit(1);}
const root=resolve(new URL('../../dist/',import.meta.url).pathname),directory=resolve(new URL('./',import.meta.url).pathname);
const child=spawn('/usr/bin/swift',['-module-cache-path','/private/tmp/soundroom-webkit-module-cache',resolve(directory,'WebKitRunner.swift'),root,resolve(directory,'fixture.js'),resolve(directory,'interactions.js')],{stdio:'inherit'});
child.on('error',error=>{console.error(error);process.exitCode=1;});child.on('exit',code=>{process.exitCode=code??1;});process.on('SIGINT',()=>child.kill());
