import { createApp } from 'vue'
import App from './App.vue'
import router from './router'
import init, { process_trade_ids } from '../../rust/pkg/hackutd'

await init()
console.log(process_trade_ids(['hello', 'wasm']))

const app = createApp(App)

app.use(router)

app.mount('#app')
