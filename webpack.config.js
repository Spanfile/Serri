'use strict'

const path = require('path')
const autoprefixer = require('autoprefixer')
const miniCssExtractPlugin = require('mini-css-extract-plugin')

module.exports = {
  mode: 'development',
  entry: {
    'index': './web/index.js',
    'device': './web/device.js'
  },
  output: {
    filename: '[name].js',
    path: path.resolve(__dirname, 'dist')
  },
  plugins: [
    new miniCssExtractPlugin()
  ],
  module: {
    rules: [
      {
        test: /\.(scss)$/,
        use: [
          {
            loader: miniCssExtractPlugin.loader
          },
          {
            loader: 'css-loader'
          },
          {
            loader: 'postcss-loader',
            options: {
              postcssOptions: {
                plugins: [
                  autoprefixer
                ]
              }
            }
          },
          {
            loader: 'sass-loader'
          }
        ]
      }
    ]
  }
}
