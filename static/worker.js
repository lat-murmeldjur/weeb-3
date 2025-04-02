import init, { Sekirei } from "./weeb_3.js";
var sekirei;

self.onconnect = async function (event) {

  await init();

  if (sekirei == undefined){
    console.log('Wings');  
    sekirei = Sekirei.new("");
    sekirei.run("");
  }

  console.log("Clouds")
     
  const port = event.ports[0];

  port.onmessage = async function (e) {
    console.log(e.data)
    if (e.data.type0 == "file") {
      var workerResultPromise0 = sekirei.post_upload(e.data.file0, e.data.encryption0);
      port.postMessage(await workerResultPromise0);
    } else if (e.data.type0 == "bootnode_settings") {
      var workerResultPromise1 = sekirei.change_bootnode_address(e.data.bootnode_address0, e.data.network_id0);
      port.postMessage(await workerResultPromise1);
    } else {
      var workerResultPromise = sekirei.acquire(e.data);
      port.postMessage(await workerResultPromise);
    }
  };
};

