markdown
深色版本
# DanbooruDownload_Tool

## 项目简介

### 中文
`DanbooruDownload_Tool` 是一个用于批量下载 Danbooru 网站上的图片及其对应标签的工具。通过简单的配置，用户可以方便地下载所需图片及其标签信息。

### English
`DanbooruDownload_Tool` is a tool for batch downloading images and their corresponding tags from the Danbooru website. With simple configuration, users can easily download the required images and their tag information.

## 安装与运行

### 中文
1. 克隆仓库：
   ```bash
   git clone https://github.com/yourusername/DanbooruDownload_Tool.git
   cd DanbooruDownload_Tool
安装依赖项：
bash
深色版本
pip install -r requirements.txt
配置文件：
编辑 config.json 文件，设置 API 密钥和其他参数。
示例配置文件：
json
深色版本
{
  "api_key": "your_api_key",
  "tags": ["tag1", "tag2"],
  "download_path": "./downloads"
}
运行工具：
bash
深色版本
python danbooru_download.py
English
Clone the repository:
bash
深色版本
git clone https://github.com/yourusername/DanbooruDownload_Tool.git
cd DanbooruDownload_Tool
Install dependencies:
bash
深色版本
pip install -r requirements.txt
Configuration file:
Edit the config.json file to set your API key and other parameters.
Example configuration file:
json
深色版本
{
  "api_key": "your_api_key",
  "tags": ["tag1", "tag2"],
  "download_path": "./downloads"
}
Run the tool:
bash
深色版本
python danbooru_download.py
依赖项
中文
requests: 用于发送 HTTP 请求。
os: 用于文件和目录操作。
json: 用于处理 JSON 数据。
English
requests: For sending HTTP requests.
os: For file and directory operations.
json: For handling JSON data.
许可证
中文
本项目采用 MIT 许可证，详情请参见 LICENSE 文件。

English
This project is licensed under the MIT License. See the LICENSE file for details.

联系方式
中文
如果你有任何问题或建议，请联系 [你的邮箱] 或在 GitHub 上提交 Issue。

English
If you have any questions or suggestions, please contact [your email] or submit an issue on GitHub.
