import os
import json
import gradio as gr
import asyncio
import aiohttp
from aiohttp import ClientSession, ClientResponseError, ClientConnectionError
import logging
from pydantic import BaseModel, Field
import time
from PIL import Image
from del_img import remove_duplicate_images_and_txt
from check_img import remove_corrupted_images_and_texts

# 配置日志系统
logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger(__name__)

# Danbooru API endpoint
base_url = "https://danbooru.donmai.us/posts.json"
config_file = "danbooru.config"


# 配置模型
class Config(BaseModel):
    username: str = Field(default="your_username", description="Danbooru 用户名")
    api_key: str = Field(default="", description="Danbooru API 密钥")
    tags: str = Field(default="", description="搜索标签，逗号分隔")
    exclude_tags: str = Field(default="", description="排除标签，逗号分隔")
    score_threshold: int = Field(default=0, description="隐藏分阈值")
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
async def fetch_posts_cached(tags, exclude_tags, score_threshold, limit, page=1, proxy=None):
    return await fetch_posts(tags, exclude_tags, score_threshold, limit, page, proxy)


async def fetch_posts(tags, exclude_tags, score_threshold, limit, page=1, proxy=None):
    params = {
        'login': config.username,
        'api_key': config.api_key,
        'tags': tags.replace(',', ' ') if tags else '',
        'limit': 200,  # 每次请求尽可能多的图片
        'page': page,
        'score': score_threshold  # 添加隐藏分参数
    }
    async with ClientSession() as session:
        if proxy:
            proxy_url = f"http://{proxy}:{config.proxy_port}"
            async with session.get(base_url, params=params, proxy=proxy_url) as response:
                if response.status == 200:
                    posts = await response.json()
                    filtered_posts = [post for post in posts if
                                      'file_url' in post and is_valid_post(post, tags, exclude_tags, score_threshold)]
                    logger.info(
                        f"Fetched {len(filtered_posts)} valid posts from page {page}. Total posts fetched: {len(posts)}")
                    return filtered_posts
                else:
                    logger.error(f"Failed to fetch posts: {response.status}")
                    return []
        else:
            async with session.get(base_url, params=params) as response:
                if response.status == 200:
                    posts = await response.json()
                    filtered_posts = [post for post in posts if
                                      'file_url' in post and is_valid_post(post, tags, exclude_tags, score_threshold)]
                    logger.info(
                        f"Fetched {len(filtered_posts)} valid posts from page {page}. Total posts fetched: {len(posts)}")
                    return filtered_posts
                else:
                    logger.error(f"Failed to fetch posts: {response.status}")
                    return []


def is_valid_post(post, tags, exclude_tags, score_threshold):
    post_tags = set(post['tag_string'].split())
    required_tags = set(tags.split(',')) if tags else set()
    excluded_tags = set(exclude_tags.split(',')) if exclude_tags else set()

    # Define allowed file extensions for images
    allowed_extensions = {'jpg', 'jpeg', 'png', 'gif'}  # Add other image extensions as necessary

    # Check if the post has a valid file extension (image)
    file_extension = post.get('file_ext', '').lower()  # Get and normalize file extension

    return (
            (not required_tags or required_tags.issubset(post_tags)) and
            (not excluded_tags or not excluded_tags.intersection(post_tags)) and
            (post.get('score', 0) >= score_threshold) and
            (file_extension in allowed_extensions)  # Ensure it's an image file
    )


def update_config(username, api_key, tags, exclude_tags, score_threshold, limit, save_path, proxy, proxy_port,
                  concurrency):
    config.username = username
    config.api_key = api_key
    config.tags = tags
    config.exclude_tags = exclude_tags
    config.score_threshold = int(score_threshold)
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
        base_filename = f"{post['id']}_{int(time.time())}"  # 使用时间戳确保文件名唯一
        filename = os.path.join(save_path, f"{base_filename}.jpg")
        tag_filename = os.path.join(save_path, f"{base_filename}.txt")
        os.makedirs(os.path.dirname(filename), exist_ok=True)
        for attempt in range(max_retries + 1):
            try:
                if proxy:
                    proxy_url = f"http://{proxy}:{config.proxy_port}"
                    async with session.get(image_url, timeout=timeout, proxy=proxy_url) as response:
                        if response.status == 200:
                            with open(filename, 'wb') as file:
                                file.write(await response.read())
                            logger.info(f"Downloaded: {filename}")
                            with open(tag_filename, 'w', encoding='utf-8') as tag_file:
                                tag_file.write(','.join(tags.split()))
                            logger.info(f"Tags saved to: {tag_filename}")
                            return filename, tags
                        else:
                            logger.warning(f"Failed to download: {image_url} (Attempt {attempt + 1}/{max_retries})")
                            if attempt < max_retries:
                                await asyncio.sleep(1)
                else:
                    async with session.get(image_url, timeout=timeout) as response:
                        if response.status == 200:
                            with open(filename, 'wb') as file:
                                file.write(await response.read())
                            logger.info(f"Downloaded: {filename}")
                            with open(tag_filename, 'w', encoding='utf-8') as tag_file:
                                tag_file.write(','.join(tags.split()))
                            logger.info(f"Tags saved to: {tag_filename}")
                            return filename, tags
                        else:
                            logger.warning(f"Failed to download: {image_url} (Attempt {attempt + 1}/{max_retries})")
                            if attempt < max_retries:
                                await asyncio.sleep(1)
            except (ClientConnectionError, ClientResponseError, aiohttp.ClientPayloadError, asyncio.TimeoutError) as e:
                logger.error(f"Connection error: {e} (Attempt {attempt + 1}/{max_retries})")
                if attempt < max_retries:
                    await asyncio.sleep(1)
        logger.error(f"Failed to download: {image_url} after multiple retries.")
        return None, None


def validate_image(filename):
    try:
        with Image.open(filename) as img:
            img.verify()
        return True
    except Exception as e:
        logger.error(f"Invalid image: {filename} - {e}")
        return False


async def download_and_preview(username, api_key, tags, exclude_tags, score_threshold, limit, save_path, proxy,
                               proxy_port, concurrency):
    update_config(username, api_key, tags, exclude_tags, score_threshold, limit, save_path, proxy, proxy_port,
                  concurrency)
    all_posts = []
    page = 1
    while len(all_posts) < limit:
        posts = await fetch_posts_cached(tags, exclude_tags, score_threshold, limit, page, proxy)
        if not posts:
            logger.warning("No more posts found matching the criteria.")
            break
        all_posts.extend(posts)
        if len(all_posts) >= limit:
            break
        page += 1
    if not all_posts:
        logger.warning("No posts found matching the criteria.")
        return [], [], "未找到符合条件的图片。"

    images = []
    tags_list = []
    failed_posts = []
    total_failed_count = 0
    initial_invalid_image_count = 0
    re_downloaded_count = 0
    final_invalid_image_count = 0
    semaphore = asyncio.Semaphore(concurrency)
    downloaded_ids = set()  # 记录已下载的图片ID

    async with ClientSession() as session:
        tasks = [download_image(session, post, save_path, semaphore, proxy=proxy) for post in all_posts[:limit] if
                 post['id'] not in downloaded_ids]
        results = await asyncio.gather(*tasks)
        for post, result in zip(all_posts[:limit], results):
            if result[0] and validate_image(result[0]):
                images.append(result[0])
                tags_list.append(result[1])
                downloaded_ids.add(post['id'])
            else:
                if result[0]:
                    os.remove(result[0])  # 删除损坏的图片
                    tag_filename = result[0].replace('.jpg', '.txt')
                    if os.path.exists(tag_filename):
                        os.remove(tag_filename)  # 删除对应的标签文件
                    initial_invalid_image_count += 1
                failed_posts.append(post)
                total_failed_count += 1
        if failed_posts:
            logger.info(f"Retrying {len(failed_posts)} failed downloads (First retry).")
            tasks = [download_image(session, post, save_path, semaphore, max_retries=10, proxy=proxy) for post in
                     failed_posts if post['id'] not in downloaded_ids]
            results = await asyncio.gather(*tasks)
            for post, result in zip(failed_posts, results):
                if result[0] and validate_image(result[0]):
                    images.append(result[0])
                    tags_list.append(result[1])
                    downloaded_ids.add(post['id'])
                    re_downloaded_count += 1
                else:
                    if result[0]:
                        os.remove(result[0])  # 删除损坏的图片
                        tag_filename = result[0].replace('.jpg', '.txt')
                        if os.path.exists(tag_filename):
                            os.remove(tag_filename)  # 删除对应的标签文件
                        final_invalid_image_count += 1
                    failed_posts.append(post)
                    total_failed_count += 1
        if len(images) < limit:
            logger.info(f"Retrying to download {limit - len(images)} more images.")
            additional_posts = all_posts[limit:limit + (limit - len(images))]
            tasks = [download_image(session, post, save_path, semaphore, proxy=proxy) for post in additional_posts if
                     post['id'] not in downloaded_ids]
            results = await asyncio.gather(*tasks)
            for post, result in zip(additional_posts, results):
                if result[0] and validate_image(result[0]):
                    images.append(result[0])
                    tags_list.append(result[1])
                    downloaded_ids.add(post['id'])
                    re_downloaded_count += 1
                else:
                    if result[0]:
                        os.remove(result[0])  # 删除损坏的图片
                        tag_filename = result[0].replace('.jpg', '.txt')
                        if os.path.exists(tag_filename):
                            os.remove(tag_filename)  # 删除对应的标签文件
                        final_invalid_image_count += 1
                    failed_posts.append(post)
                    total_failed_count += 1
        # 确保最终的图片数量不超过设定的限制
        images = images[:limit]
        tags_list = tags_list[:limit]

    if images:
        logger.info(f"Downloaded {len(images)} images.")
        logger.info(f"Total failed downloads: {total_failed_count}")
        logger.info(f"Initial invalid images: {initial_invalid_image_count}")
        logger.info(f"Final invalid images: {final_invalid_image_count}")
        logger.info(f"Total re-downloaded images: {re_downloaded_count}")
        deleted_images_count,del_log = remove_duplicate_images_and_txt(save_path)
        deleted_images = remove_corrupted_images_and_texts(save_path)
        logger.info(del_log)
        logger.info(f"删除图片{deleted_images_count}张")
        return images, tags_list, f"实际下载 {len(images)-deleted_images_count} 张图片，{total_failed_count} 张图片下载失败，初始损坏图片 {initial_invalid_image_count} 张，最终损坏图片 {final_invalid_image_count+deleted_images} 张，重新下载了 {re_downloaded_count} 张图片，删除图片重复{deleted_images_count}张。"
    else:
        logger.warning("No images downloaded.")
        return [], [], "没有下载任何图片。"


def prepare_gallery(images, tags_list):
    gallery_items = []
    for image_path, tags in zip(images, tags_list):
        gallery_items.append((image_path, tags))
    return gallery_items


def delete_images_by_tag(tag, save_path):
    deleted_count = 0
    for root, _, files in os.walk(save_path):
        for file in files:
            if file.endswith('.txt'):
                tag_file_path = os.path.join(root, file)
                with open(tag_file_path, 'r', encoding='utf-8') as tag_file:
                    tags = tag_file.read().strip().split(',')
                    if tag in tags:
                        image_file = file.replace('.txt', '.jpg')
                        image_path = os.path.join(root, image_file)
                        tag_path = os.path.join(root, file)
                        if os.path.exists(image_path):
                            try:
                                os.remove(image_path)
                                logger.info(f"Deleted image: {image_path}")
                            except PermissionError as e:
                                logger.error(f"Failed to delete image: {image_path} - {e}")
                                time.sleep(1)  # 等待1秒后重试
                                try:
                                    os.remove(image_path)
                                    logger.info(f"Deleted image (retry): {image_path}")
                                except PermissionError as e:
                                    logger.error(f"Failed to delete image (retry): {image_path} - {e}")
                        if os.path.exists(tag_path):
                            try:
                                os.remove(tag_path)
                                logger.info(f"Deleted tag file: {tag_path}")
                            except PermissionError as e:
                                logger.error(f"Failed to delete tag file: {tag_path} - {e}")
                                time.sleep(1)  # 等待1秒后重试
                                try:
                                    os.remove(tag_path)
                                    logger.info(f"Deleted tag file (retry): {tag_path}")
                                except PermissionError as e:
                                    logger.error(f"Failed to delete tag file (retry): {tag_path} - {e}")
                        deleted_count += 1
    return f"已删除 {deleted_count}张带有标签 '{tag}' 的图片及其标签文件。"


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
with gr.Blocks(css=custom_css, theme='YTheme/Minecraft') as demo:
    gr.Markdown("# Danbooru 图片下载器")
    with gr.Row():
        username_input = gr.Textbox(label="用户名", value=config.username)
        api_key_input = gr.Textbox(label="API 密钥", value=config.api_key, type="password")
    with gr.Row():
        tags_input = gr.Textbox(label="标签 (逗号分隔)", value=config.tags)
        exclude_tags_input = gr.Textbox(label="排除标签 (逗号分隔)", value=config.exclude_tags)
    with gr.Row():
        score_threshold_input = gr.Slider(label="隐藏分阈值", minimum=0, maximum=1000, step=1,
                                          value=config.score_threshold)
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
        gallery_output = gr.Gallery(label="图片预览", columns=4, height="auto", elem_id="gallery")

    def on_update_config(username, api_key, tags, exclude_tags, score_threshold, limit, save_path, proxy, proxy_port,
                         concurrency):
        update_config(username, api_key, tags, exclude_tags, score_threshold, limit, save_path, proxy, proxy_port,
                      concurrency)
        return "配置已更新成功。"


    def on_download_and_preview(username, api_key, tags, exclude_tags, score_threshold, limit, save_path, proxy,
                                proxy_port, concurrency):
        loop = asyncio.new_event_loop()
        asyncio.set_event_loop(loop)
        images, tags_list, status = loop.run_until_complete(
            download_and_preview(username, api_key, tags, exclude_tags, score_threshold, limit, save_path, proxy,
                                 proxy_port, concurrency))
        return status, prepare_gallery(images, tags_list)


    def on_delete_images_by_tag(tag, save_path):
        return delete_images_by_tag(tag, save_path)


    # 绑定更新配置按钮
    update_button.click(
        fn=on_update_config,
        inputs=[username_input, api_key_input, tags_input, exclude_tags_input, score_threshold_input, limit_input,
                save_path_input, proxy_input, proxy_port_input, concurrency_input],
        outputs=[status_output]
    )

    # 绑定下载并预览按钮
    download_button.click(
        fn=on_download_and_preview,
        inputs=[username_input, api_key_input, tags_input, exclude_tags_input, score_threshold_input, limit_input,
                save_path_input, proxy_input, proxy_port_input, concurrency_input],
        outputs=[status_output, gallery_output]
    )
demo.launch()
