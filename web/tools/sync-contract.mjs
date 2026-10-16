import {readFile,writeFile} from 'node:fs/promises';
const canonical=new URL('../../protocol/',import.meta.url);
for(const file of ['generated/models.ts','generated/contract.ts','generated/write-ranges.ts','validate.ts','write_validate.ts']){
 let text=await readFile(new URL(file,canonical),'utf8');
 if(!file.startsWith('generated/'))for(const module of ['generated/contract','generated/models','generated/write-ranges','write_validate'])text=text.replaceAll(`'./${module}'`,`'./${module}.js'`);
 await writeFile(new URL('../src/'+file,import.meta.url),text);
}
