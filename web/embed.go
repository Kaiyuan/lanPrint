package web

import "embed"

//go:embed index.html favicon.ico i18n/*.json static/css/*.css static/js/*.js
var Files embed.FS
