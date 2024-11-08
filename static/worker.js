console.log('Initializing worker')
import init, { Sekirei } from "./weeb_3.js";

await init();

var sekirei = Sekirei.new("");
console.log('Initialized worker');
sekirei.run("");
