import os
import json
import gradio as gr
import asyncio
import aiohttp
from aiohttp import ClientSession, ClientResponseError, ClientConnectionError
import logging
from pydantic import BaseModel, Field

# 配置日志系统
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)

# Danbooru API endpoint
base_url = "https://danbooru.donmai.us/posts.json"
config_file = "danbooru.config"

# 配置模型
class Config(BaseModel):
    username: str = Field(default="your_username", description="Danbooru 用户名")
    api_key: str = Field(default="Xie2sjwmXDo3me9JWJ8VUKMF", description="Danbooru API 密钥")
    tags: str = Field(default="", description="搜索标签，逗号分隔")
    exclude_tags: str = Field(default="", description="排除标签，逗号分隔")
    limit: int = Field(default=10, description="下载数量")
    save_path: str = Field(default="danbooru_images", description="保存路径")
    proxy: str = Field(default="", description="代理服务器")
    proxy_port: str = Field(default="", description="代理服务器端口")
    concurrency: int = Field(default=5, description="并发下载数量")

# 读取配置文件
def load_config():
    if os.path.exists(config_file):
        with open(config_file, 'r') as f:
            config = json.load(f)
    else:
        config = Config().model_dump()
    return Config(**config)

# 保存配置文件
def save_config(config: Config):
    with open(config_file, 'w') as f:
        json.dump(config.model_dump(), f, indent=4)

# 加载配置
config = load_config()

# 缓存 API 响应
async def fetch_posts_cached(tags, exclude_tags, limit, page=1, proxy=None):
    return await fetch_posts(tags, exclude_tags, limit, page, proxy)

async def fetch_posts(tags, exclude_tags, limit, page=1, proxy=None):
    params = {
        'login': config.username,
        'api_key': config.api_key,
        'tags': tags.replace(',', ' ') if tags else '',
        'limit': limit,
        'page': page
    }
    async with ClientSession() as session:
        if proxy:
            proxy_url = f"http://{proxy}:{config.proxy_port}"
            async with session.get(base_url, params=params, proxy=proxy_url) as response:
                if response.status == 200:
                    posts = await response.json()
                    filtered_posts = [post for post in posts if 'file_url' in post and is_valid_post(post, tags, exclude_tags)]
                    logger.info(f"Fetched {len(filtered_posts)} valid posts from page {page}. Total posts fetched: {len(posts)}")
                    return filtered_posts
                else:
                    logger.error(f"Failed to fetch posts: {response.status}")
                    return []
        else:
            async with session.get(base_url, params=params) as response:
                if response.status == 200:
                    posts = await response.json()
                    filtered_posts = [post for post in posts if 'file_url' in post and is_valid_post(post, tags, exclude_tags)]
                    logger.info(f"Fetched {len(filtered_posts)} valid posts from page {page}. Total posts fetched: {len(posts)}")
                    return filtered_posts
                else:
                    logger.error(f"Failed to fetch posts: {response.status}")
                    return []

def is_valid_post(post, tags, exclude_tags):
    post_tags = set(post['tag_string'].split())
    required_tags = set(tags.split(',')) if tags else set()
    excluded_tags = set(exclude_tags.split(',')) if exclude_tags else set()
    return (not required_tags or required_tags.issubset(post_tags)) and (not excluded_tags or not excluded_tags.intersection(post_tags))

def update_config(username, api_key, tags, exclude_tags, limit, save_path, proxy, proxy_port, concurrency):
    config.username = username
    config.api_key = api_key
    config.tags = tags
    config.exclude_tags = exclude_tags
    config.limit = int(limit)
    config.save_path = save_path
    config.proxy = proxy
    config.proxy_port = proxy_port
    config.concurrency = int(concurrency)
    save_config(config)
    logger.info("Configuration updated successfully.")
    return "配置已更新成功。"

async def download_image(session, post, save_path, semaphore, max_retries=5, timeout=30, proxy=None):
    async with semaphore:
        image_url = post['file_url']
        tags = post['tag_string']
        filename = os.path.join(save_path, f"{post['id']}.jpg")
        
        # 确保目录存在
        os.makedirs(os.path.dirname(filename), exist_ok=True)
        
        # 下载图片
        for attempt in range(max_retries + 1):
            try:
                if proxy:
                    proxy_url = f"http://{proxy}:{config.proxy_port}"
                    async with session.get(image_url, timeout=timeout, proxy=proxy_url) as response:
                        if response.status == 200:
                            with open(filename, 'wb') as file:
                                file.write(await response.read())
                            logger.info(f"Downloaded: {filename}")
                            
                            # 保存标签
                            tag_filename = os.path.join(save_path, f"{post['id']}.txt")
                            with open(tag_filename, 'w', encoding='utf-8') as tag_file:
                                tag_file.write(','.join(tags.split()))
                            logger.info(f"Tags saved to: {tag_filename}")
                            return filename, tags
                        else:
                            logger.warning(f"Failed to download: {image_url} (Attempt {attempt + 1}/{max_retries})")
                            if attempt < max_retries:
                                await asyncio.sleep(1)  # 等待1秒后重试
                else:
                    async with session.get(image_url, timeout=timeout) as response:
                        if response.status == 200:
                            with open(filename, 'wb') as file:
                                file.write(await response.read())
                            logger.info(f"Downloaded: {filename}")
                            
                            # 保存标签
                            tag_filename = os.path.join(save_path, f"{post['id']}.txt")
                            with open(tag_filename, 'w', encoding='utf-8') as tag_file:
                                tag_file.write(','.join(tags.split()))
                            logger.info(f"Tags saved to: {tag_filename}")
                            return filename, tags
                        else:
                            logger.warning(f"Failed to download: {image_url} (Attempt {attempt + 1}/{max_retries})")
                            if attempt < max_retries:
                                await asyncio.sleep(1)  # 等待1秒后重试
            except (ClientConnectionError, ClientResponseError, aiohttp.ClientPayloadError, asyncio.TimeoutError) as e:
                logger.error(f"Connection error: {e} (Attempt {attempt + 1}/{max_retries})")
                if attempt < max_retries:
                    await asyncio.sleep(1)  # 等待1秒后重试
        logger.error(f"Failed to download: {image_url} after multiple retries.")
        return None, None

async def download_and_preview(username, api_key, tags, exclude_tags, limit, save_path, proxy, proxy_port, concurrency):
    update_config(username, api_key, tags, exclude_tags, limit, save_path, proxy, proxy_port, concurrency)
    all_posts = []
    page_limit = 100  # 每页最多请求 100 张图片
    remaining_limit = limit
    page = 1
    
    while remaining_limit > 0:
        current_limit = min(page_limit, remaining_limit)
        posts = await fetch_posts_cached(tags, exclude_tags, current_limit, page, proxy)
        if not posts:
            logger.warning("No more posts found matching the criteria.")
            break
        all_posts.extend(posts)
        remaining_limit -= len(posts)
        page += 1
    
    if not all_posts:
        logger.warning("No posts found matching the criteria.")
        return [], [], "未找到符合条件的图片。"
    
    images = []
    tags_list = []
    failed_posts = []
    total_failed_count = 0  # 记录总失败次数
    
    semaphore = asyncio.Semaphore(concurrency)  # 控制并发下载数量
    async with ClientSession() as session:
        while len(images) < limit:
            tasks = [download_image(session, post, save_path, semaphore, proxy=proxy) for post in all_posts]
            results = await asyncio.gather(*tasks)
            
            for result in results:
                if result[0]:
                    images.append(result[0])
                    tags_list.append(result[1])
                else:
                    failed_posts.append(post)
                    total_failed_count += 1  # 记录失败次数
            
            # 第一轮重试失败的图片
            if failed_posts:
                logger.info(f"Retrying {len(failed_posts)} failed downloads (First retry).")
                tasks = [download_image(session, post, save_path, semaphore, max_retries=10, proxy=proxy) for post in failed_posts]
                results = await asyncio.gather(*tasks)
                
                for result in results:
                    if result[0]:
                        images.append(result[0])
                        tags_list.append(result[1])
                    else:
                        failed_posts.append(post)
                        total_failed_count += 1  # 记录失败次数
            
            # 记录最终失败的图片数量
            final_failed_count = len(failed_posts)
            if final_failed_count > 0:
                logger.warning(f"{final_failed_count} images failed to download after multiple retries.")
            
            # 如果有失败的图片，尝试下载额外的图片来填补空缺
            if final_failed_count > 0:
                additional_posts = await fetch_posts_cached(tags, exclude_tags, final_failed_count, page, proxy)
                if additional_posts:
                    logger.info(f"Fetching {final_failed_count} additional images to fill gaps.")
                    all_posts.extend(additional_posts)
                    failed_posts.clear()
                else:
                    logger.warning("No more additional images to fetch.")
                    break
    
    if images:
        logger.info(f"Downloaded {len(images)} images.")
        logger.info(f"Total failed downloads: {total_failed_count}")
        return images, tags_list, f"已下载 {len(images)} 张图片，{total_failed_count} 张图片下载失败。"
    else:
        logger.warning("No images downloaded.")
        return [], [], "没有下载任何图片。"

def prepare_gallery(images, tags_list):
    gallery_items = []
    for image_path, tags in zip(images, tags_list):
        gallery_items.append((image_path, tags))
    return gallery_items

# 自定义 CSS 样式
custom_css = """
<style>
.gr-gallery-container {
    display: flex;
    flex-wrap: wrap;
}
.gr-gallery-item {
    flex: 1 1 calc(25% - 10px); /* 每行显示 4 张图片 */
    min-width: 150px; /* 最小宽度 */
    margin: 5px; /* 添加外边距 */
}
</style>
"""

# Gradio界面
with gr.Blocks(css=custom_css) as demo:
    gr.Markdown("# Danbooru 图片下载器")
    
    with gr.Row():
        username_input = gr.Textbox(label="用户名", value=config.username)
        api_key_input = gr.Textbox(label="API 密钥", value=config.api_key, type="password")
    
    with gr.Row():
        tags_input = gr.Textbox(label="标签 (逗号分隔)", value=config.tags)
        exclude_tags_input = gr.Textbox(label="排除标签 (逗号分隔)", value=config.exclude_tags)
    
    with gr.Row():
        limit_input = gr.Number(label="下载数量", value=config.limit, precision=0)
        save_path_input = gr.Textbox(label="保存路径 (默认为当前目录下的 danbooru_images)", value=config.save_path)
    
    with gr.Row():
        proxy_input = gr.Textbox(label="代理服务器", value=config.proxy)
        proxy_port_input = gr.Textbox(label="代理服务器端口", value=config.proxy_port)
    
    with gr.Row():
        concurrency_input = gr.Slider(label="并发下载数量", minimum=1, maximum=20, step=1, value=config.concurrency)
    
    with gr.Row():
        update_button = gr.Button("更新配置")
        download_button = gr.Button("下载并预览")
    
    with gr.Row():
        status_output = gr.Textbox(label="状态")

    with gr.Row():
        gallery_output = gr.Gallery(label="图片预览", columns=4, height="auto", elem_id="gallery")  # 设置高度为 auto

    def on_update_config(username, api_key, tags, exclude_tags, limit, save_path, proxy, proxy_port, concurrency):
        update_config(username, api_key, tags, exclude_tags, limit, save_path, proxy, proxy_port, concurrency)
        return "配置已更新成功。"

    def on_download_and_preview(username, api_key, tags, exclude_tags, limit, save_path, proxy, proxy_port, concurrency):
        loop = asyncio.new_event_loop()
        asyncio.set_event_loop(loop)
        images, tags_list, status = loop.run_until_complete(download_and_preview(username, api_key, tags, exclude_tags, limit, save_path, proxy, proxy_port, concurrency))
        return status, prepare_gallery(images, tags_list)

    # 绑定更新配置按钮
    update_button.click(
        fn=on_update_config,
        inputs=[username_input, api_key_input, tags_input, exclude_tags_input, limit_input, save_path_input, proxy_input, proxy_port_input, concurrency_input],
        outputs=[status_output]
    )

    # 绑定下载并预览按钮
    download_button.click(
        fn=on_download_and_preview,
        inputs=[username_input, api_key_input, tags_input, exclude_tags_input, limit_input, save_path_input, proxy_input, proxy_port_input, concurrency_input],
        outputs=[status_output, gallery_output]
    )

demo.launch()