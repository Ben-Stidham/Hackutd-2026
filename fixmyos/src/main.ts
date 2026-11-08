import './assets/main.css'
import { createApp } from 'vue'
import App from './App.vue'
import router from './router'
import { getErrors } from './ws'

const app = createApp(App)

app.use(router)

app.mount('#app')
getErrors().catch(console.error)
