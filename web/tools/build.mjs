import {copyFile} from 'node:fs/promises';
for(const file of ['index.html','style.css'])await copyFile(new URL('../'+file,import.meta.url),new URL('../dist/'+file,import.meta.url));
console.log('Built static assets in web/dist; serve through the authenticated Soundroom HTTPS host.');
